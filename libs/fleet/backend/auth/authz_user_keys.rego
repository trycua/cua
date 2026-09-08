# /api/user-keys and /api/user-keys/{id} — create, list and revoke personal API
# keys.
#
# Interactive clients only. A user key must not manage user keys: that would let
# a leaked key mint replacements for itself and revoke the ones a human would use
# to shut it off.
package authz_user_keys

import data.authz

default allow = false

allow {
	input.route == "/api/user-keys"
	authz.is_interactive_client
}

allow {
	input.route == "/api/user-keys/{id}"
	authz.is_interactive_client
}
