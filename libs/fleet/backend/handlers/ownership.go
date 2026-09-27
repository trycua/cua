// The Kubernetes half of namespace ownership: does this subject hold RBAC in
// this namespace?
//
// It used to be a whole authorization decision made here, because the policy
// had no way to ask Kubernetes anything. It is now one input to a decision the
// policy makes — authz_ownership.rego — reaching it through the FactProvider
// below. What stayed is the probe; what left is every judgement about which
// principals deserve one.
//
// The probe is the impersonated RoleBinding list already proven by
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

	"golang.org/x/sync/singleflight"

	"cyclops-cs-backend/auth"
)

// ownershipSF collapses concurrent probes for the same (sub, ns) — noVNC
// fires dozens of parallel asset requests on first load, and each is its own
// request, so the policy library's per-request fact cache cannot merge them.
// Singleflight holds nothing between requests: once the shared probe returns,
// the next request asks the apiserver again. Mirrors flagsSF in
// auth/middlewares.go; package-level like the k8sImpersonate client state —
// Handlers is a value type, so per-struct state wouldn't survive across
// requests anyway.
var ownershipSF singleflight.Group

// NamespaceRBACFacts adapts the probe below to the policy layer. It is the
// third of the three decisions requireNamespaceAccess used to make in Go: the
// other two were claim checks over the token, and authz_ownership.rego makes
// them now. This one is not a rule at all — it is a fact about Kubernetes — so
// it arrives as input.facts.namespace_rbac instead.
//
// The verdict is cached per request, by the policy library's fact cache, and
// no longer than that: every request that reaches the fact leaf asks the
// apiserver, so a revoked or re-created namespace is seen on the very next
// request rather than after a TTL.
//
// main.go registers it under auth.NamespaceRBACFactProvider, which is the name
// NamespaceOwnershipPolicy's tree refers to.
func NamespaceRBACFacts(h Handlers) auth.FactProvider {
	return namespaceRBACFacts{handlers: h}
}

type namespaceRBACFacts struct{ handlers Handlers }

func (namespaceRBACFacts) CacheKey() string { return auth.NamespaceRBACFactProvider }

// LoadFacts answers whether this request's subject holds RBAC in this request's
// namespace, and nothing else. It deliberately does not ask who the caller is:
// which principals are worth probing is a policy question, and
// authz_ownership.rego's probe_eligible is where it is answered. What is left
// here is the pair of degenerate inputs the probe cannot form a question from —
// no subject to impersonate, or no namespace to ask about — which deny without
// a round trip.
func (facts namespaceRBACFacts) LoadFacts(ctx context.Context, r *http.Request) (auth.FactSet, error) {
	user := auth.GetUser(r.Context())
	namespace := auth.OwnedNamespace(r.Context())
	if user == nil || user.ID == "" || namespace == "" {
		return auth.FactSet{"allowed": false}, nil
	}

	allowed, err := facts.handlers.userHasNamespaceRBAC(ctx, user.ID, namespace)
	if err != nil {
		slog.Warn("namespace access check unavailable",
			"class", "dependency_unavailable", "retryable", true,
			"sub", user.ID, "azp", user.AZP, "namespace", namespace)
		return nil, auth.NewFactUnavailableError(auth.NamespaceRBACFactNamespace, err)

	}
	if !allowed {
		// The verdict is the policy's to reach, but this is the only place the
		// apiserver's "no" is observed, and it was a logged event before the
		// check moved. PolicyMiddleware's 403 names a route, not a namespace.
		slog.Warn("namespace access denied",
			"sub", user.ID, "azp", user.AZP, "namespace", namespace)
	}
	return auth.FactSet{"allowed": allowed}, nil
}

// userHasNamespaceRBAC reports whether sub holds RBAC in ns, via a
// singleflighted impersonated RoleBinding LIST probe. Nothing is retained
// across requests: concurrent probes for the same (sub, ns) share one round
// trip, and sequential requests each make their own. A non-nil error means
// the apiserver was unreachable or replied with an unexpected status —
// callers fail closed.
func (h Handlers) userHasNamespaceRBAC(ctx context.Context, sub, ns string) (bool, error) {
	key := sub + "\x00" + ns

	v, err, _ := ownershipSF.Do(key, func() (interface{}, error) {
		resp, err := h.k8sImpersonate(ctx, "GET",
			"/apis/rbac.authorization.k8s.io/v1/namespaces/"+url.PathEscape(ns)+"/rolebindings?limit=1",
			nil, sub)
		if err != nil {
			return false, err
		}
		defer resp.Body.Close()

		switch {
		case resp.StatusCode >= 200 && resp.StatusCode < 300:
			return true, nil
		case resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusForbidden:
			return false, nil
		default:
			// 5xx etc. — indeterminate, fail closed.
			return false, &unexpectedProbeStatus{status: resp.StatusCode}
		}
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
