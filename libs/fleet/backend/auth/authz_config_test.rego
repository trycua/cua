# What /api/config runs: All(authz_base.allow, authz_config.allow).
#
# route_allow below is that composition, so every case asserts the verdict the
# route produces rather than one module's half of it. Splitting the policy must
# not turn a test of the whole check into a test of a fragment.
package authz_config_test

import rego.v1

import data.authz_base
import data.authz_config

route_allow if {
	authz_base.allow
	authz_config.allow
}

test_user_api_key_config_allowed if {
	route_allow with input as {
		"route": "/api/config",
		"method": "GET",
		"path": "/api/config",
		"params": {},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

test_spa_analytics_session_allowed if {
	route_allow with input as {
		"route": "/api/analytics/session",
		"method": "POST",
		"path": "/api/analytics/session",
		"params": {},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
	}
}

test_user_key_analytics_session_denied if {
	not route_allow with input as {
		"route": "/api/analytics/session",
		"method": "POST",
		"path": "/api/analytics/session",
		"params": {},
		"user": {"sub": "user-123", "azp": "ukey-test", "namespace": "", "email": ""},
	}
}

test_spa_analytics_attribution_allowed if {
	route_allow with input as {
		"route": "/api/analytics/attribution",
		"method": "POST",
		"path": "/api/analytics/attribution",
		"params": {},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
	}
}

test_user_key_analytics_attribution_denied if {
	not route_allow with input as {
		"route": "/api/analytics/attribution",
		"method": "POST",
		"path": "/api/analytics/attribution",
		"params": {},
		"user": {"sub": "user-123", "azp": "ukey-test", "namespace": "", "email": ""},
	}
}
