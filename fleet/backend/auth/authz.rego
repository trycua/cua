# Cyclops-CS authorization policy.
#
# Surfaces, distinguished by `input.route`:
#
#   /api/keys, /api/k8s, /api/orch — SPA surfaces. Require a token
#                       issued to the cyclops-cs-spa client
#                       (azp == "cyclops-cs-spa") and a non-empty
#                       subject. Anyone signed into the cyclops-cs realm
#                       may use them.
#
#   /api/batch/{pool}, /api/label/{pool}
#                     — DEPRECATED. The handler returns 410 Gone for
#                       every request. These rules remain so that
#                       unauthenticated callers still receive 401/403
#                       rather than 410, preserving the auth boundary
#                       even for a retired surface.
#
#   /api/gateway/{name} — orchestrator proxy. Requires a token issued to
#                         a per-key client (azp starts with "key-") with:
#                           • a non-empty `namespace` claim
#                           • `namespace` == "{name}" (the token's
#                             hardcoded-claim mapper must name the exact pool
#                             the client belongs to, so one compromised key
#                             cannot reach another pool)
#                         Per-pool orchestrators no longer have a direct
#                         Tailscale Ingress — all traffic MUST flow through
#                         this proxy (CUA-527).
#
# Data-visibility filtering (which events/env-vars the caller may see) lives
# in filters.rego, not here.  authz.rego decides allow/deny only.
package authz

default allow = false

# ── Admin helper ────────────────────────────────────────────────────────────
#
# is_admin is true when the calling user's JWT sub is in the admin set.
# Flags arrive via `input.flags` (resolved per-request from the OpenFeature
# provider and TTL-cached in auth/middlewares.go), NOT a static OPA data
# store — so admin-membership changes take effect within flagsTTL without a
# pod restart. filters.rego reuses this via `data.authz.is_admin`.
is_admin {
    input.flags.admin_subs[_] == input.user.sub
}

# ── Route allow rules ───────────────────────────────────────────────────────

# /api/keys (POST/GET) and /api/keys/{id} (DELETE).
allow {
    input.route == "/api/keys"
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
}

allow {
    input.route == "/api/keys/{id}"
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
}

# /api/config — per-user feature flags. SPA-only.
allow {
    input.route == "/api/config"
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
}

# /api/namespaces — namespace management (list, create, delete).
# SPA-only; any authenticated user may manage their own namespaces.
# Capsule enforces per-user scoping at the K8s API level via
# impersonation headers added by the backend.
allow {
    input.route == "/api/namespaces"
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
}

allow {
    input.route == "/api/namespaces/{name}"
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
}

# /api/pool-templates — per-user saved pool configs (list, create, get,
# delete). SPA-only; the backend scopes every operation to the caller's
# sub, so a user can only ever read or mutate their own templates.
allow {
    input.route == "/api/pool-templates"
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
}

allow {
    input.route == "/api/pool-templates/{name}"
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
}

# /api/k8s/{path...} — kubectl-proxy passthrough.
#
# Any SPA user may read customer-facing resources (their pools, the pods
# backing their VMs, pod logs, metrics, and the K8s event feed — the event
# *contents* are still redacted for non-admins by filters.rego's
# visible_events). Infra-only resources (cluster nodes, cluster-wide pod
# listings, batch Jobs, and the app's own cyclops-cs ConfigMaps) require
# admin — hiding them is enforced HERE, not just in the SPA nav. CUA-515.
allow {
    input.route == "/api/k8s/{path...}"
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
    not is_infra_k8s_path(input.params.path)
}

allow {
    input.route == "/api/k8s/{path...}"
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
    is_admin
}

# is_infra_k8s_path matches the kubectl-proxy paths that expose cluster
# internals. `input.params.path` is the captured {path...} segment with no
# leading slash (e.g. "api/v1/nodes"); query strings are not included.
#
#   api/v1/nodes[/...]                       — NodesList (cluster capacity)
#   api/v1/pods                              — cluster-wide pod listing
#                                              (namespaced pods stay open)
#   apis/batch/v1/...                        — batch Jobs
#   api/v1/namespaces/cyclops-cs/configmaps  — the app's own config
#   apis/cua.ai/v1/osgymworkspacepools       — cluster-wide pool listing
#                                              (namespaced pools stay open)
#   apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims
#                                            — cluster-wide claim listing
#                                              (namespaced claims stay open)
#
# The two cluster-wide CRD lists are defence-in-depth: the SPA only ever reads
# pools/claims per-namespace, and tenant RBAC no longer grants the cluster-wide
# list (see clusters/kopf-k3s/capsule-rbac/rbac.yaml). Blocking them here keeps
# a non-admin from enumerating every tenant's pools/claims through the proxy.
is_infra_k8s_path(path) { path == "api/v1/nodes" }
is_infra_k8s_path(path) { startswith(path, "api/v1/nodes/") }
is_infra_k8s_path(path) { path == "api/v1/pods" }
is_infra_k8s_path(path) { startswith(path, "apis/batch/v1") }
is_infra_k8s_path(path) { startswith(path, "api/v1/namespaces/cyclops-cs/configmaps") }
is_infra_k8s_path(path) { path == "apis/cua.ai/v1/osgymworkspacepools" }
is_infra_k8s_path(path) { path == "apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims" }

# /api/orch/{namespace}/{service}/{path...} — per-namespace orchestrator
# catalog read. SPA-only; namespace + service must be DNS-1123 labels.
# Like /api/svc, this proxy dials Service DNS directly, so namespace
# ownership is enforced in Go (handlers.requireNamespaceAccess).
allow {
    input.route == "/api/orch/{namespace}/{service}/{path...}"
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
    valid_dns_label(input.params.namespace)
    valid_dns_label(input.params.service)
}

# /api/batch/{pool}/* and /api/label/{pool}/* — DEPRECATED (410 Gone).
#
# These routes are permanently retired; the handler (DeprecatedBatchRoute)
# returns 410 for every request that passes auth. The allow rules below are
# retained so that unauthenticated or unauthorized callers still see
# 401/403 rather than 410, keeping the auth boundary intact on a dead
# surface. No business logic is exercised after OPA passes.
allow {
    is_batch_route
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
    valid_dns_label(input.params.pool)
}
allow {
    is_batch_route
    input.user.sub != ""
    is_per_key_client
    valid_dns_label(input.params.pool)
    input.user.namespace == input.params.pool
}

is_batch_route { input.route == "/api/batch/{pool}/submit" }
is_batch_route { input.route == "/api/batch/{pool}/lanes" }
is_batch_route { input.route == "/api/batch/{pool}/{id}/status" }
is_batch_route { input.route == "/api/batch/{pool}/{id}/results" }
is_batch_route { input.route == "/api/batch/{pool}/{id}" }
is_batch_route { input.route == "/api/label/{pool}/{label}/batch" }
is_batch_route { input.route == "/api/label/{pool}/{label}/status" }
is_batch_route { input.route == "/api/label/{pool}/{label}/results" }
is_batch_route { input.route == "/api/label/{pool}/{label}" }

# /api/gateway/{name}[/{path...}].
#
# The JWT `namespace` claim is set by a hardcoded-claim protocol mapper on
# the per-key Keycloak client.  Pool name = namespace name (1:1), so we
# enforce the exact match here: a token for pool "foo" carries
# namespace="foo" and may only reach /api/gateway/foo/…
allow {
    is_gateway_route
    is_per_key_client
    valid_dns_label(input.params.name)
    input.user.namespace != ""
    input.user.namespace == input.params.name
}

is_gateway_route { input.route == "/api/gateway/{name}" }
is_gateway_route { input.route == "/api/gateway/{name}/{path...}" }

# /api/svc/{namespace}/{service}[/{path...}] — generic service proxy.
#
# OPA gates authentication + parameter shape ONLY. The proxy dials
# {service}.{namespace}.svc.cluster.local directly — it never traverses
# the K8s API — so Capsule CANNOT scope it, and OPA has no K8s data to
# check ownership itself. Namespace ownership is enforced in Go
# (handlers.requireNamespaceAccess) via an impersonated RoleBinding
# probe before any byte is proxied.
#
# Per-key clients: token's namespace claim must match the {namespace}
# param (enforced both here and in the handler).
allow {
    is_svc_route
    is_per_key_client
    input.user.sub != ""
    valid_dns_label(input.params.namespace)
    valid_dns_label(input.params.service)
    input.user.namespace == input.params.namespace
}
# Every other principal carries a real user identity in `sub`, and the
# handler's impersonated RoleBinding probe is the authorization boundary
# — so OPA only authenticates and shape-checks. This covers:
#   • SPA users (azp == "cyclops-cs-spa")
#   • user-keys (azp == "ukey-*"; sub overridden to the key owner)
#   • oauth2-proxy cookie sessions (synthetic azp == "oauth2-proxy")
#   • MCP connector clients minted via RFC 7591 dynamic client
#     registration — their azp is a per-registration UUID, so it cannot
#     be enumerated here
# Per-key clients are excluded: their rule above binds them to their
# namespace claim, and their sub is a service account that owns nothing.
allow {
    is_svc_route
    not is_per_key_client
    input.user.sub != ""
    valid_dns_label(input.params.namespace)
    valid_dns_label(input.params.service)
}

is_svc_route { input.route == "/api/svc/{namespace}/{service}" }
is_svc_route { input.route == "/api/svc/{namespace}/{service}/{path...}" }

is_per_key_client { startswith(input.user.azp, "key-") }

# ── Per-user API keys (/api/user-keys) ─────────────────────────────────
#
# Create / list / revoke personal API keys. SPA-only — users manage
# their own keys through the dashboard.
allow {
    input.route == "/api/user-keys"
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
}

allow {
    input.route == "/api/user-keys/{id}"
    input.user.sub != ""
    input.user.azp == "cyclops-cs-spa"
}

# ── User API key access to SPA surfaces ────────────────────────────────
#
# User-key tokens are Keycloak client_credentials JWTs with azp starting
# with "ukey-". The middleware overrides sub with the key owner's
# identity before OPA runs, so input.user.sub is the real user. These
# keys get the same access as SPA users for programmatic automation.

is_user_key_client { startswith(input.user.azp, "ukey-") }

# Namespace management
allow {
    input.route == "/api/namespaces"
    input.user.sub != ""
    is_user_key_client
}

allow {
    input.route == "/api/namespaces/{name}"
    input.user.sub != ""
    is_user_key_client
}

# Pool templates — user API keys manage their owner's templates, same as
# SPA users (the backend scopes every call to the resolved sub).
allow {
    input.route == "/api/pool-templates"
    input.user.sub != ""
    is_user_key_client
}

allow {
    input.route == "/api/pool-templates/{name}"
    input.user.sub != ""
    is_user_key_client
}

# K8s API proxy — user API keys get the same access as non-admin SPA users
# (infra paths are blocked).
allow {
    input.route == "/api/k8s/{path...}"
    input.user.sub != ""
    is_user_key_client
    not is_infra_k8s_path(input.params.path)
}

# Orchestrator catalog access
allow {
    input.route == "/api/orch/{namespace}/{service}/{path...}"
    input.user.sub != ""
    is_user_key_client
    valid_dns_label(input.params.namespace)
    valid_dns_label(input.params.service)
}

# Batch endpoints — user API keys may target any pool (like SPA users).
# Routes are deprecated and return 410 Gone; rules kept for auth boundary.
allow {
    is_batch_route
    input.user.sub != ""
    is_user_key_client
    valid_dns_label(input.params.pool)
}

# Config endpoint
allow {
    input.route == "/api/config"
    input.user.sub != ""
    is_user_key_client
}

# DNS-1123 label: lowercase alphanumerics + dashes, 1..63 chars,
# can't start or end with a dash. Belt-and-braces against a misconfigured
# Keycloak mapper or a request that slipped past the Go regex.
valid_dns_label(s) {
    s != ""
    count(s) <= 63
    regex.match(`^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`, s)
}
