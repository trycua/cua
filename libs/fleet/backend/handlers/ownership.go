// Namespace-ownership enforcement for the direct-dial service proxies
// (/api/svc and /api/orch).
//
// These proxies dial {service}.{namespace}.svc.cluster.local directly —
// they never traverse the K8s API — so Capsule cannot enforce tenancy on
// them the way it does for /api/k8s. OPA can't either: it has no K8s
// data. This file is the tenancy boundary for those routes.
//
// The check is the impersonated RoleBinding probe already proven by
// waitForNamespaceAdoption (namespaces.go): listing RoleBindings in a
// namespace is only authorized for principals holding RBAC there — i.e.
// the Capsule tenant owner. (A namespace GET would NOT work: the
// capsule-tenant-cluster-resources ClusterRole grants namespaces
// get/list cluster-wide to system:authenticated, so it gates nothing.)
package handlers

import (
	"context"
	"fmt"
	"log/slog"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"golang.org/x/sync/singleflight"

	"cyclops-cs-backend/auth"
)

const (
	// ownershipPositiveTTL bounds how long an "owns it" verdict is reused.
	// Ownership effectively never changes, but a short TTL keeps a deleted
	// + re-created namespace from serving a stale verdict for long, while
	// still collapsing the noVNC asset storm to ~2 probes/min/user/ns.
	ownershipPositiveTTL = 30 * time.Second
	// ownershipNegativeTTL damps brute-force namespace enumeration without
	// locking a user out of a just-adopted namespace for long.
	ownershipNegativeTTL = 5 * time.Second
)

type ownershipVerdict struct {
	allowed bool
	exp     time.Time
}

// Package-level like the k8sImpersonate client state — Handlers is a value
// type, so per-struct state wouldn't survive across requests anyway.
var (
	ownershipMu    sync.Mutex
	ownershipCache = map[string]ownershipVerdict{}
	// ownershipSF collapses concurrent probes for the same (sub, ns) —
	// noVNC fires dozens of parallel asset requests on first load.
	// Mirrors flagsSF in auth/middlewares.go.
	ownershipSF singleflight.Group
)

// resetOwnershipCache clears the verdict cache; tests only.
func resetOwnershipCache() {
	ownershipMu.Lock()
	ownershipCache = map[string]ownershipVerdict{}
	ownershipMu.Unlock()
}

// keyClientPfx returns the per-key (pool) client-id prefix, defaulting to
// "key-" when config is zero-valued (tests construct Handlers{} bare).
func (h Handlers) keyClientPfx() string {
	if h.AuthCfg.KeyClientPfx != "" {
		return h.AuthCfg.KeyClientPfx
	}
	return "key-"
}

func (h Handlers) userKeyClientPfx() string {
	if h.AuthCfg.UserKeyClientPfx != "" {
		return h.AuthCfg.UserKeyClientPfx
	}
	return "ukey-"
}

// isPerKeyClient reports whether the token was minted for a per-pool key
// client. ukey-* (per-user key) clients carry a real user identity and
// must NOT match — guard against prefix overlap ("ukey-" does not start
// with "key-", but keep the explicit exclusion in case the configured
// prefixes ever nest).
func (h Handlers) isPerKeyClient(user *auth.User) bool {
	if strings.HasPrefix(user.AZP, h.userKeyClientPfx()) {
		return false
	}
	return strings.HasPrefix(user.AZP, h.keyClientPfx())
}

// requireNamespaceAccess authorizes user to reach Services in ns through
// the direct-dial proxies. Per-key tokens must carry namespace claim ==
// ns (defense-in-depth mirror of OPA); every other principal (SPA, ukey,
// oauth2-proxy) must hold RBAC in ns, verified by a cached impersonated
// RoleBinding probe. On deny/error it writes the response and returns
// false; callers must return immediately.
func (h Handlers) requireNamespaceAccess(w http.ResponseWriter, r *http.Request, user *auth.User, ns string) bool {
	// Per-key clients: bound to exactly one namespace by their token
	// claim. No K8s call — the claim IS the grant.
	if h.isPerKeyClient(user) {
		if user.Namespace != "" && user.Namespace == ns {
			return true
		}
		slog.Warn("namespace access denied (per-key claim mismatch)",
			"sub", user.ID, "azp", user.AZP, "claim", user.Namespace, "namespace", ns)
		writeErr(w, http.StatusForbidden, "namespace access denied")
		return false
	}

	allowed, err := h.userHasNamespaceRBAC(r.Context(), user.ID, ns)
	if err != nil {
		slog.Warn("namespace access check unavailable",
			"sub", user.ID, "azp", user.AZP, "namespace", ns, "err", err)
		writeErr(w, http.StatusBadGateway, "authorization check unavailable")
		return false
	}
	if !allowed {
		slog.Warn("namespace access denied",
			"sub", user.ID, "azp", user.AZP, "namespace", ns)
		writeErr(w, http.StatusForbidden, "namespace access denied")
		return false
	}
	return true
}

// userHasNamespaceRBAC reports whether sub holds RBAC in ns, via a TTL-
// cached, singleflighted impersonated RoleBinding LIST probe. A non-nil
// error means the apiserver was unreachable or replied with an unexpected
// status — callers fail closed; nothing is cached so a flapping apiserver
// can't pin verdicts.
func (h Handlers) userHasNamespaceRBAC(ctx context.Context, sub, ns string) (bool, error) {
	key := sub + "\x00" + ns

	ownershipMu.Lock()
	if v, ok := ownershipCache[key]; ok && time.Now().Before(v.exp) {
		ownershipMu.Unlock()
		return v.allowed, nil
	}
	ownershipMu.Unlock()

	v, err, _ := ownershipSF.Do(key, func() (interface{}, error) {
		// Re-check under the lock: a concurrent probe may have just
		// populated the cache while we waited to enter singleflight.
		ownershipMu.Lock()
		if v, ok := ownershipCache[key]; ok && time.Now().Before(v.exp) {
			ownershipMu.Unlock()
			return v.allowed, nil
		}
		ownershipMu.Unlock()

		resp, err := h.k8sImpersonate("GET",
			"/apis/rbac.authorization.k8s.io/v1/namespaces/"+url.PathEscape(ns)+"/rolebindings?limit=1",
			nil, sub)
		if err != nil {
			return false, err
		}
		defer resp.Body.Close()

		var allowed bool
		switch {
		case resp.StatusCode >= 200 && resp.StatusCode < 300:
			allowed = true
		case resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusForbidden:
			allowed = false
		default:
			// 5xx etc. — indeterminate, fail closed, don't cache.
			return false, &unexpectedProbeStatus{status: resp.StatusCode}
		}

		ttl := ownershipPositiveTTL
		if !allowed {
			ttl = ownershipNegativeTTL
		}
		ownershipMu.Lock()
		ownershipCache[key] = ownershipVerdict{allowed: allowed, exp: time.Now().Add(ttl)}
		ownershipMu.Unlock()
		return allowed, nil
	})
	if err != nil {
		return false, err
	}
	return v.(bool), nil
}

type unexpectedProbeStatus struct{ status int }

func (e *unexpectedProbeStatus) Error() string {
	return fmt.Sprintf("unexpected status %d from rolebinding probe", e.status)
}
