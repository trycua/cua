# Cyclops-CS principal vocabulary.
#
# This module answers "who is calling", and nothing else. It holds no allow
# rule and no route name: what a principal may do on a given surface is decided
# by that surface's own module (authz_keys.rego, authz_k8s.rego, …), each of
# which imports this one.
#
# The split is what keeps route authorization honest. Until it existed, one
# module carried 30 route-gated allow rules and routes picked it up ad hoc, so a
# route could leave main.go with its rules still in place — which is exactly what
# happened when the /api/gateway route was deleted in #6702. A surface module is
# named by the routes that run it (see routeSurfaces in auth/policy_routes.go),
# so rules for a retired surface have nowhere to hide.
#
# Every route composes All(authz_base.allow, <surface>.allow):
# authz_base.rego requires a non-empty subject, which was the one condition all
# 30 rules shared, and the surface decides the rest. See policy_routes.go.
#
# is_admin is defined here because its two consumers sit on opposite sides of
# the split: pool_admission.rego, a conjunct over the request body, and
# auth.EvalIsAdmin, which answers off a User rather than a request. No surface
# module reads it any more — authz_k8s.rego did, as an escape hatch over its
# infra paths, until #6704 turned that surface into an allowlist and left admins
# with the same access as everyone else.
package authz

# is_admin is true when the calling user's JWT sub is in the admin set.
# Flags arrive via `input.flags` (resolved per-request from the OpenFeature
# provider and TTL-cached in auth/middlewares.go), NOT a static OPA data
# store — so admin-membership changes take effect within flagsTTL without a
# pod restart.
is_admin {
	input.flags.admin_subs[_] == input.user.sub
}

# chat_enabled is the restricted-mode decision. Global all/disabled mode is
# configuration state and remains outside policy; this rule only decides whether
# an authenticated user belongs to the administrator-or-chat rollout set.
chat_enabled {
	is_admin
}

chat_enabled {
	input.flags.chat_subs[_] == input.user.sub
}

# Usage preview is restricted to administrators and the initial internal rollout set.
usage_enabled {
	is_admin
}

usage_enabled {
	input.flags.usage_subs[_] == input.user.sub
}

# ── Token families ──────────────────────────────────────────────────────────
#
# Three families reach the policy, distinguished by `azp`. A fourth kind of
# caller — oauth2-proxy cookie sessions and MCP connector clients registered
# through RFC 7591 — matches none of these predicates, which is deliberate: their
# azp cannot be enumerated, so surfaces that accept them say so by not requiring
# a family rather than by naming one.

# Exact interactive clients. Keep this allowlist closed: accepting any
# arbitrary public realm client would let unrelated clients call user APIs.
is_interactive_client {
	input.user.azp == "cyclops-cs-spa"
}

is_interactive_client {
	input.user.azp == "cua-cli"
}

# Per-key clients are Keycloak service accounts bound to one namespace by a
# hardcoded `namespace` claim. Their sub owns nothing on its own, so every
# surface that accepts them checks that claim against the route's parameters.
is_per_key_client {
	key_client_prefix := object.get(input.user, "key_client_prefix", "key-")
	key_client_prefix != ""
	startswith(input.user.azp, key_client_prefix)
}

# A default-prefix service account must not fall through as an ordinary user
# when the deployment has moved per-key clients to another configured prefix.
is_legacy_per_key_client {
	key_client_prefix := object.get(input.user, "key_client_prefix", "key-")
	key_client_prefix != "key-"
	startswith(input.user.azp, "key-")
}

# User-key tokens are Keycloak client_credentials JWTs with azp starting with
# "ukey-". The middleware overrides sub with the key owner's identity before the
# policy runs, so input.user.sub is the real user and these keys get SPA-like
# access for programmatic automation.
is_user_key_client {
	startswith(input.user.azp, "ukey-")
}

# The verified form additionally requires the principal_type the middleware
# stamps only after resolving the owner. A token whose azp merely looks like a
# user key does not qualify.
is_verified_user_key_client {
	is_user_key_client
	input.user.principal_type == "user_key"
}

# ── Parameter shapes ────────────────────────────────────────────────────────

# DNS-1123 label: lowercase alphanumerics + dashes, 1..63 chars,
# can't start or end with a dash. Belt-and-braces against a misconfigured
# Keycloak mapper or a request that slipped past the Go regex.
valid_dns_label(s) {
	s != ""
	count(s) <= 63
	regex.match(`^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`, s)
}
