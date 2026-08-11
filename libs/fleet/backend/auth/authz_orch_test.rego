# What /api/orch/{namespace}/{service}/{path...} runs: All(authz_base.allow, authz_orch.allow).
#
# route_allow below is that composition, so every case asserts the verdict the
# route produces rather than one module's half of it. Splitting the policy must
# not turn a test of the whole check into a test of a fragment.
package authz_orch_test

import rego.v1

import data.authz_base
import data.authz_orch

route_allow if {
	authz_base.allow
	authz_orch.allow
}

test_orch_spa_allowed if {
	route_allow with input as {
		"route": "/api/orch/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/orch/mypool/mypool-orchestrator/status",
		"params": {"namespace": "mypool", "service": "mypool-orchestrator", "path": "status"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_orch_cua_cli_allowed if {
	route_allow with input as {
		"route": "/api/orch/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/orch/mypool/mypool-orchestrator/status",
		"params": {"namespace": "mypool", "service": "mypool-orchestrator", "path": "status"},
		"user": {"sub": "user-123", "azp": "cua-cli", "namespace": "", "email": "u@example.com"},
	}
}

test_user_api_key_orch_allowed if {
	route_allow with input as {
		"route": "/api/orch/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/orch/mypool/mypool-orchestrator/status",
		"params": {"namespace": "mypool", "service": "mypool-orchestrator", "path": "status"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}
