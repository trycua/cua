# /api/keys (POST/GET) and /api/keys/{id} (DELETE) — CRUD on the per-pool
# service-account clients in Keycloak.
#
# Exact interactive clients only. A per-key client must not be able to mint more
# per-key clients, and a user key must not either: key issuance is a dashboard
# action, so it stays on a token a human obtained interactively.
package authz_keys

import data.authz

default allow = false

allow {
	input.route == "/api/keys"
	authz.is_interactive_client
}

allow {
	input.route == "/api/keys/{id}"
	authz.is_interactive_client
}
