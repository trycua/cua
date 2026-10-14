# What /api/user-keys and /api/user-keys/{id} runs: All(authz_base.allow, authz_user_keys.allow).
#
# route_allow below is that composition, so every case asserts the verdict the
# route produces rather than one module's half of it. Splitting the policy must
# not turn a test of the whole check into a test of a fragment.
package authz_user_keys_test

import rego.v1

import data.authz_base
import data.authz_user_keys

route_allow if {
	authz_base.allow
	authz_user_keys.allow
}

test_user_keys_spa_create_allowed if {
	route_allow with input as {
		"route": "/api/user-keys",
		"method": "POST",
		"path": "/api/user-keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_user_keys_spa_list_allowed if {
	route_allow with input as {
		"route": "/api/user-keys",
		"method": "GET",
		"path": "/api/user-keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_user_keys_spa_delete_allowed if {
	route_allow with input as {
		"route": "/api/user-keys/{id}",
		"method": "DELETE",
		"path": "/api/user-keys/abc123",
		"params": {"id": "abc123"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

# User-key clients cannot manage other user API keys (must use SPA).
test_user_keys_api_key_denied if {
	not route_allow with input as {
		"route": "/api/user-keys",
		"method": "GET",
		"path": "/api/user-keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

# Per-key clients cannot manage user API keys.
test_user_keys_per_key_denied if {
	not route_allow with input as {
		"route": "/api/user-keys",
		"method": "POST",
		"path": "/api/user-keys",
		"params": {},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

# Empty sub denied on user-key management routes.
test_user_keys_empty_sub_denied if {
	not route_allow with input as {
		"route": "/api/user-keys",
		"method": "GET",
		"path": "/api/user-keys",
		"params": {},
		"user": {"sub": "", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
	}
}

# Missing azp denied on user-key management routes.
test_user_keys_missing_azp_denied if {
	not route_allow with input as {
		"route": "/api/user-keys",
		"method": "POST",
		"path": "/api/user-keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "", "namespace": "", "email": "u@example.com"},
	}
}

# Per-key client cannot delete user API keys.
test_user_keys_per_key_delete_denied if {
	not route_allow with input as {
		"route": "/api/user-keys/{id}",
		"method": "DELETE",
		"path": "/api/user-keys/abc123",
		"params": {"id": "abc123"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

# User-key client cannot delete user API keys (only SPA).
test_user_keys_user_key_delete_denied if {
	not route_allow with input as {
		"route": "/api/user-keys/{id}",
		"method": "DELETE",
		"path": "/api/user-keys/abc123",
		"params": {"id": "abc123"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}
