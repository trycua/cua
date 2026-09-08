# /api/state/query — read-only state SQL.
#
# PostgreSQL row-level security remains the row-visibility boundary; this module
# decides only who may ask.
#
# The method check is part of the surface, not incidental: the route is
# registered as QUERY only, so a request arriving under any other method reached
# it some other way and is denied here as well as by the mux.
package authz_state_query

import data.authz

default allow = false

allow {
	input.route == "/api/state/query"
	input.method == "QUERY"
	authz.is_interactive_client
}

allow {
	input.route == "/api/state/query"
	input.method == "QUERY"
	authz.is_user_key_client
}
