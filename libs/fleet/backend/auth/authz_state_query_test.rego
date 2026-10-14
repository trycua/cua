# What /api/state/query runs: All(authz_base.allow, authz_state_query.allow).
#
# route_allow below is that composition, so every case asserts the verdict the
# route produces rather than one module's half of it. Splitting the policy must
# not turn a test of the whole check into a test of a fragment.
package authz_state_query_test

import rego.v1

import data.authz_base
import data.authz_state_query

route_allow if {
	authz_base.allow
	authz_state_query.allow
}

test_state_query_spa_allowed if {
	route_allow with input as {
		"route": "/api/state/query",
		"method": "QUERY",
		"path": "/api/state/query",
		"params": {},
		"user": {"sub": "alice", "azp": "cyclops-cs-spa"},
	}
}

test_state_query_user_key_allowed if {
	route_allow with input as {
		"route": "/api/state/query",
		"method": "QUERY",
		"path": "/api/state/query",
		"params": {},
		"user": {"sub": "alice", "azp": "ukey-alice"},
	}
}

test_state_query_untrusted_client_denied if {
	not route_allow with input as {
		"route": "/api/state/query",
		"method": "QUERY",
		"path": "/api/state/query",
		"params": {},
		"user": {"sub": "alice", "azp": "untrusted"},
	}
}

test_state_query_empty_subject_denied if {
	not route_allow with input as {
		"route": "/api/state/query",
		"method": "QUERY",
		"path": "/api/state/query",
		"params": {},
		"user": {"sub": "", "azp": "cyclops-cs-spa"},
	}
}

test_state_query_post_denied if {
	not route_allow with input as {
		"route": "/api/state/query",
		"method": "POST",
		"path": "/api/state/query",
		"params": {},
		"user": {"sub": "alice", "azp": "cyclops-cs-spa"},
	}
}
