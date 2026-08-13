# /api/orch/{namespace}/{service}/{path...} — per-namespace orchestrator
# catalog read.
#
# Like /api/svc, this proxy dials Service DNS directly rather than traversing the
# K8s API, so Capsule cannot scope it. This module authenticates the family and
# shape-checks the parameters; namespace ownership is the third conjunct of
# OrchRoutePolicy, authz_ownership.rego, which reads the impersonated RoleBinding
# probe as input.facts.namespace_rbac.
#
# Per-key clients match neither rule below, so they never reach that conjunct:
# their sub is a service account that owns nothing, and their namespace claim is
# not what this surface is scoped by.
package authz_orch

import data.authz

default allow = false

allow {
	input.route == "/api/orch/{namespace}/{service}/{path...}"
	authz.is_interactive_client
	authz.valid_dns_label(input.params.namespace)
	authz.valid_dns_label(input.params.service)
}

allow {
	input.route == "/api/orch/{namespace}/{service}/{path...}"
	authz.is_user_key_client
	authz.valid_dns_label(input.params.namespace)
	authz.valid_dns_label(input.params.service)
}
