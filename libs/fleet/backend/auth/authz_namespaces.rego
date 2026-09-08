# /api/namespaces and /api/namespaces/{name} — namespace management
# (list, create, get, delete).
#
# Three principal families reach this surface, and none of them is scoped here:
# Capsule enforces per-user ownership at the K8s API level, via the impersonation
# headers the backend adds. So these rules authenticate the family and leave
# ownership to Capsule.
#
# With one exception, which is not in this module: GET /api/namespaces/{name}
# also runs the ownership conjunct (authz_ownership.rego), because it used to
# run the same check in Go after reading the namespace back. The other three
# routes on this surface pass that conjunct unconditionally.
#
# Per-key clients are absent on purpose. Their sub is a service account that owns
# no namespace, so impersonating it would produce an empty list rather than a
# denial, which reads as "you have none" instead of "you may not ask".
package authz_namespaces

import data.authz

default allow = false

allow {
	input.route == "/api/namespaces"
	authz.is_interactive_client
}

allow {
	input.route == "/api/namespaces/{name}"
	authz.is_interactive_client
}

allow {
	input.route == "/api/namespaces"
	input.user.principal_type == "github_oidc"
}

allow {
	input.route == "/api/namespaces/{name}"
	input.user.principal_type == "github_oidc"
}

allow {
	input.route == "/api/namespaces"
	authz.is_user_key_client
}

allow {
	input.route == "/api/namespaces/{name}"
	authz.is_user_key_client
}
