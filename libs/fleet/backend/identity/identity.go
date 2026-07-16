// Package identity resolves the cross-service identity prefixes (Keycloak
// group / Capsule tenant prefix and the OIDC username/groups prefixes) from
// OpenFeature, so the cyclops-cs backend, the kopf Capsule provisioner, and the
// k3s apiserver flags share ONE source of truth (AWS SSM) instead of each
// hardcoding its own copy — the drift that previously broke tenant RBAC.
//
// Flag keys (SSM Parameter Store paths, same keys the kopf provisioner reads):
//
//	/feature-flags/identity/user-group-prefix     default "user-"
//	/feature-flags/identity/oidc-username-prefix   default "oidc:"
//	/feature-flags/identity/oidc-groups-prefix     default "oidc:"
//	/feature-flags/identity/tenant-group           default "cyclops-cs-tenants"
//
// Defaults equal today's hardcoded literals, so behaviour is unchanged until an
// SSM value is set. The OpenFeature provider must be installed
// (featureflags.SetupProvider) before these are called; with no provider the
// SDK returns the supplied default.
//
// Callers should use the Impersonate*/Tenant* composers rather than
// concatenating prefixes themselves, so the composition rules live in one place.
package identity

import (
	"context"
	"sync"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

const (
	flagUserGroupPrefix     = "/feature-flags/identity/user-group-prefix"
	flagOIDCUsernamePrefix  = "/feature-flags/identity/oidc-username-prefix"
	flagOIDCGroupsPrefix    = "/feature-flags/identity/oidc-groups-prefix"
	flagTenantGroup         = "/feature-flags/identity/tenant-group"
	defaultUserGroupPrefix  = "user-"
	defaultOIDCUserPrefix   = "oidc:"
	defaultOIDCGroupsPrefix = "oidc:"
	// defaultTenantGroup is the shared Capsule "gate" group every cyclops user is
	// impersonated into. It is the single, STATIC entry in
	// CapsuleConfiguration.userGroups (declared in GitOps at
	// clusters/kopf-k3s/capsule/capsule-configuration.yaml) — keep the two in
	// sync. Per-user isolation is the Tenant owner (= the impersonated User),
	// not this group, so userGroups never needs per-user runtime patching.
	defaultTenantGroup = "cyclops-cs-tenants"
	// cacheTTL bounds how long a resolved prefix is reused before re-resolving
	// via OpenFeature (mirrors auth.flagsTTL). Refresh is lazy on the next call.
	cacheTTL = 1 * time.Minute
	// evalTimeout keeps an unreachable Parameter Store from stalling a request;
	// the SDK falls back to the default value on timeout.
	evalTimeout = 3 * time.Second
)

var (
	clientOnce sync.Once
	client     *openfeature.Client

	mu     sync.Mutex
	cache  = map[string]cached{}
)

type cached struct {
	val string
	exp time.Time
}

func ff() *openfeature.Client {
	clientOnce.Do(func() { client = openfeature.NewClient("cyclops-cs-identity") })
	return client
}

// resolve returns the flag's string value, cached for cacheTTL, falling back to
// def on error/timeout/missing provider.
func resolve(ctx context.Context, key, def string) string {
	mu.Lock()
	if c, ok := cache[key]; ok && time.Now().Before(c.exp) {
		mu.Unlock()
		return c.val
	}
	mu.Unlock()

	callCtx, cancel := context.WithTimeout(ctx, evalTimeout)
	defer cancel()
	val, err := ff().StringValue(callCtx, key, def, openfeature.EvaluationContext{})
	if err != nil || val == "" {
		val = def
	}

	mu.Lock()
	cache[key] = cached{val: val, exp: time.Now().Add(cacheTTL)}
	mu.Unlock()
	return val
}

// UserGroupPrefix is the Keycloak personal-group / Capsule tenant prefix ("user-").
func UserGroupPrefix(ctx context.Context) string {
	return resolve(ctx, flagUserGroupPrefix, defaultUserGroupPrefix)
}

// OIDCUsernamePrefix is the apiserver oidc-username-prefix ("oidc:").
func OIDCUsernamePrefix(ctx context.Context) string {
	return resolve(ctx, flagOIDCUsernamePrefix, defaultOIDCUserPrefix)
}

// OIDCGroupsPrefix is the apiserver oidc-groups-prefix ("oidc:").
func OIDCGroupsPrefix(ctx context.Context) string {
	return resolve(ctx, flagOIDCGroupsPrefix, defaultOIDCGroupsPrefix)
}

// ImpersonateUser builds the K8s Impersonate-User value for a Keycloak subject
// (e.g. "oidc:<sub>").
func ImpersonateUser(ctx context.Context, sub string) string {
	return OIDCUsernamePrefix(ctx) + sub
}

// PersonalGroup is the Keycloak personal group / Capsule tenant name for a
// subject (e.g. "user-<sub>").
func PersonalGroup(ctx context.Context, sub string) string {
	return UserGroupPrefix(ctx) + sub
}

// TenantGroup is the shared Capsule gate group, oidc-groups prefix applied
// (e.g. "oidc:cyclops-cs-tenants"). It is the single static entry in
// CapsuleConfiguration.userGroups.
func TenantGroup(ctx context.Context) string {
	return OIDCGroupsPrefix(ctx) + resolve(ctx, flagTenantGroup, defaultTenantGroup)
}

// ImpersonateGroup builds the K8s Impersonate-Group value: the shared Capsule
// gate group (e.g. "oidc:cyclops-cs-tenants"). Every cyclops user is
// impersonated as a member of this ONE group so Capsule treats the request as
// tenant-managed; which tenant they own is decided by the Tenant owner matching
// ImpersonateUser (the User), NOT by a per-user group. The sub arg is unused
// (kept for call-site stability / future per-user gating).
func ImpersonateGroup(ctx context.Context, _ string) string {
	return TenantGroup(ctx)
}
