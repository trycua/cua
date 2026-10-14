# /api/svc/{namespace}/{service}[/{path...}] — generic service proxy.
#
# This module gates authentication + parameter shape. The proxy dials
# {service}.{namespace}.svc.cluster.local directly — it never traverses the K8s
# API — so Capsule CANNOT scope it. Namespace ownership is the third conjunct of
# SvcRoutePolicy, authz_ownership.rego, which reads the impersonated RoleBinding
# probe as input.facts.namespace_rbac.
package authz_svc

import data.authz

default allow = false

# Per-key clients: the token's namespace claim must match the {namespace} param.
# authz_ownership.rego says the same thing for the same principals; this is the
# rule that keeps a per-key token from reaching the ownership conjunct at all,
# and the duplication is what makes the surface readable on its own.
allow {
	is_svc_route
	authz.is_per_key_client
	authz.valid_dns_label(input.params.namespace)
	authz.valid_dns_label(input.params.service)
	input.user.namespace == input.params.namespace
}

# Every other principal carries a real user identity in `sub`, and the ownership
# conjunct is the authorization boundary — so this only authenticates and
# shape-checks. It covers:
#   • SPA users (azp == "cyclops-cs-spa")
#   • user-keys (azp == "ukey-*"; sub overridden to the key owner)
#   • oauth2-proxy cookie sessions (synthetic azp == "oauth2-proxy")
#   • MCP connector clients minted via RFC 7591 dynamic client registration —
#     their azp is a per-registration UUID, so it cannot be enumerated here
#
# Per-key clients are excluded rather than folded in: the rule above binds them
# to their namespace claim, and their sub is a service account that owns nothing,
# so a catch-all that included them would hand them every namespace.
allow {
	is_svc_route
	not authz.is_per_key_client
	authz.valid_dns_label(input.params.namespace)
	authz.valid_dns_label(input.params.service)
}

is_svc_route {
	input.route == "/api/svc/{namespace}/{service}"
}

is_svc_route {
	input.route == "/api/svc/{namespace}/{service}/{path...}"
}
