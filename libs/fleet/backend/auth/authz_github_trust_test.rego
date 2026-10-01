# What the GitHub trust-policy routes runs: All(authz_base.allow, authz_github_trust.allow).
#
# route_allow below is that composition, so every case asserts the verdict the
# route produces rather than one module's half of it. Splitting the policy must
# not turn a test of the whole check into a test of a fragment.
package authz_github_trust_test

import rego.v1

import data.authz_base
import data.authz_github_trust

route_allow if {
	authz_base.allow
	authz_github_trust.allow
}

test_user_api_key_github_trust_policies_allowed if {
	route_allow with input as {
		"route": "/api/github-trust-policies",
		"method": "POST",
		"path": "/api/github-trust-policies",
		"params": {},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": "", "principal_type": "user_key"},
	}
}

test_user_api_key_github_trust_policy_allowed if {
	route_allow with input as {
		"route": "/api/github-trust-policies/{id}",
		"method": "PATCH",
		"path": "/api/github-trust-policies/policy-123",
		"params": {"id": "policy-123"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": "", "principal_type": "user_key"},
	}
}

test_unverified_user_key_github_trust_policies_denied if {
	not route_allow with input as {
		"route": "/api/github-trust-policies",
		"method": "POST",
		"path": "/api/github-trust-policies",
		"params": {},
		"user": {"sub": "service-account-ukey-rogue", "azp": "ukey-rogue", "namespace": "", "email": "", "principal_type": "user"},
	}
}

test_per_key_github_trust_policies_denied if {
	not route_allow with input as {
		"route": "/api/github-trust-policies",
		"method": "POST",
		"path": "/api/github-trust-policies",
		"params": {},
		"user": {"sub": "svc-123", "azp": "key-pool-foo", "namespace": "pool-foo", "email": ""},
	}
}

test_per_key_github_trust_policy_denied if {
	not route_allow with input as {
		"route": "/api/github-trust-policies/{id}",
		"method": "DELETE",
		"path": "/api/github-trust-policies/policy-123",
		"params": {"id": "policy-123"},
		"user": {"sub": "svc-123", "azp": "key-pool-foo", "namespace": "pool-foo", "email": ""},
	}
}

test_untrusted_client_github_trust_policies_denied if {
	not route_allow with input as {
		"route": "/api/github-trust-policies",
		"method": "GET",
		"path": "/api/github-trust-policies",
		"params": {},
		"user": {"sub": "user-123", "azp": "untrusted-client", "namespace": "", "email": ""},
	}
}

test_untrusted_client_github_trust_policy_denied if {
	not route_allow with input as {
		"route": "/api/github-trust-policies/{id}",
		"method": "GET",
		"path": "/api/github-trust-policies/policy-123",
		"params": {"id": "policy-123"},
		"user": {"sub": "user-123", "azp": "untrusted-client", "namespace": "", "email": ""},
	}
}
