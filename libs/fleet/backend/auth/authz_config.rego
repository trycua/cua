# /api/config — the per-user feature flags the SPA reads at load.
#
# Interactive clients and user keys. The response is scoped to the caller's own
# sub, so a user key seeing it sees its owner's flags.
package authz_config

import data.authz

default allow = false

allow {
	input.route == "/api/config"
	authz.is_interactive_client
}

allow {
	input.route == "/api/config"
	authz.is_user_key_client
}

allow {
	input.route == "/api/analytics/session"
	input.user.azp == "cyclops-cs-spa"
}

allow {
	input.route == "/api/analytics/attribution"
	input.method == "POST"
	input.user.azp == "cyclops-cs-spa"
}
