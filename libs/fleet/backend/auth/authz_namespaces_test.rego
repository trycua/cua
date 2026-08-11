# What /api/namespaces and /api/namespaces/{name} runs: All(authz_base.allow, authz_namespaces.allow).
#
# route_allow below is that composition, so every case asserts the verdict the
# route produces rather than one module's half of it. Splitting the policy must
# not turn a test of the whole check into a test of a fragment.
package authz_namespaces_test

import rego.v1

import data.authz_base
import data.authz_namespaces

route_allow if {
	authz_base.allow
	authz_namespaces.allow
}

test_namespaces_spa_list_allowed if {
	route_allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"path": "/api/namespaces",
		"params": {},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_namespaces_spa_create_allowed if {
	route_allow with input as {
		"route": "/api/namespaces/{name}",
		"method": "POST",
		"path": "/api/namespaces/my-ns",
		"params": {"name": "my-ns"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_namespaces_spa_delete_allowed if {
	route_allow with input as {
		"route": "/api/namespaces/{name}",
		"method": "DELETE",
		"path": "/api/namespaces/my-ns",
		"params": {"name": "my-ns"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

# Per-key clients (key-*) CANNOT access namespace routes.
test_namespaces_per_key_list_denied if {
	not route_allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"path": "/api/namespaces",
		"params": {},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

test_namespaces_per_key_by_name_denied if {
	not route_allow with input as {
		"route": "/api/namespaces/{name}",
		"method": "DELETE",
		"path": "/api/namespaces/my-ns",
		"params": {"name": "my-ns"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

# Empty sub denied on namespace routes.
test_namespaces_empty_sub_denied if {
	not route_allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"path": "/api/namespaces",
		"params": {},
		"user": {"sub": "", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

# Missing azp denied on namespace routes.
test_namespaces_missing_azp_denied if {
	not route_allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"path": "/api/namespaces",
		"params": {},
		"user": {"sub": "user-123", "azp": "", "namespace": "", "email": "u@example.com"},
	}
}

test_user_api_key_namespaces_allowed if {
	route_allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"path": "/api/namespaces",
		"params": {},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

test_user_api_key_namespaces_delete_allowed if {
	route_allow with input as {
		"route": "/api/namespaces/{name}",
		"method": "DELETE",
		"path": "/api/namespaces/my-ns",
		"params": {"name": "my-ns"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

# User-key clients with empty sub are denied.
test_user_api_key_no_sub_denied if {
	not route_allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"path": "/api/namespaces",
		"params": {},
		"user": {"sub": "", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}
