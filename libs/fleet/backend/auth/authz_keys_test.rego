# What /api/keys and /api/keys/{id} runs: All(authz_base.allow, authz_keys.allow).
#
# route_allow below is that composition, so every case asserts the verdict the
# route produces rather than one module's half of it. Splitting the policy must
# not turn a test of the whole check into a test of a fragment.
package authz_keys_test

import rego.v1

import data.authz_base
import data.authz_keys

route_allow if {
	authz_base.allow
	authz_keys.allow
}

test_keys_spa_allowed if {
	route_allow with input as {
		"route": "/api/keys",
		"method": "POST",
		"path": "/api/keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_keys_cua_cli_allowed if {
	route_allow with input as {
		"route": "/api/keys",
		"method": "POST",
		"path": "/api/keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "cua-cli", "namespace": "", "email": "u@example.com"},
	}
}

test_keys_other_interactive_client_denied if {
	not route_allow with input as {
		"route": "/api/keys",
		"method": "POST",
		"path": "/api/keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "kubernetes", "namespace": "", "email": "u@example.com"},
	}
}

test_keys_non_spa_denied if {
	not route_allow with input as {
		"route": "/api/keys",
		"method": "POST",
		"path": "/api/keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "key-some-pool", "namespace": "some-pool", "email": "u@example.com"},
	}
}
