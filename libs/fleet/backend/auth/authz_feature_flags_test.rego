package authz_feature_flags_test

import rego.v1

import data.authz_base
import data.authz_feature_flags

route_allow if {
	authz_base.allow
	authz_feature_flags.allow
}

test_admin_spa_allowed if {
	route_allow with input as input_for({"sub": "admin-1", "azp": "cyclops-cs-spa"})
}

test_cli_denied if {
	not route_allow with input as input_for({"sub": "admin-1", "azp": "cua-cli"})
}

test_user_key_denied if {
	not route_allow with input as input_for({"sub": "admin-1", "azp": "ukey-abc", "principal_type": "user_key"})
}

test_github_oidc_denied if {
	not route_allow with input as input_for({"sub": "admin-1", "azp": "github-actions"})
}

test_oauth2_proxy_denied if {
	not route_allow with input as input_for({"sub": "admin-1", "azp": "oauth2-proxy"})
}

test_empty_subject_denied if {
	not route_allow with input as input_for({"sub": "", "azp": "cyclops-cs-spa"})
}

test_ordinary_spa_denied if {
	not route_allow with input as input_for({"sub": "ordinary-user", "azp": "cyclops-cs-spa"})
}

input_for(user) := {
	"route": "/api/admin/feature-flags/{key}",
	"method": "PUT",
	"path": "/api/admin/feature-flags/example",
	"params": {"key": "example"},
	"user": object.union({"namespace": "", "email": "", "principal_type": ""}, user),
	"flags": {"admin_subs": ["admin-1"]},
}
