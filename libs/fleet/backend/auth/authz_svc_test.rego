# What /api/svc/{namespace}/{service}[/{path...}] runs: All(authz_base.allow, authz_svc.allow).
#
# route_allow below is that composition, so every case asserts the verdict the
# route produces rather than one module's half of it. Splitting the policy must
# not turn a test of the whole check into a test of a fragment.
#
# The catch-all rule accepts principals whose azp cannot be enumerated —
# oauth2-proxy sessions and RFC 7591 dynamically registered MCP clients — so the
# cases below pin both that it accepts them and that it does not thereby relax
# the per-key namespace binding.
package authz_svc_test

import rego.v1

import data.authz_base
import data.authz_svc

route_allow if {
	authz_base.allow
	authz_svc.allow
}

test_svc_per_key_matching_namespace_allowed if {
	route_allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/mypool/mypool-orchestrator/status",
		"params": {"namespace": "mypool", "service": "mypool-orchestrator", "path": "status"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}

test_svc_per_key_no_trailing_path_allowed if {
	route_allow with input as {
		"route": "/api/svc/{namespace}/{service}",
		"method": "GET",
		"path": "/api/svc/mypool/my-svc",
		"params": {"namespace": "mypool", "service": "my-svc"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}

test_svc_per_key_wrong_namespace_denied if {
	not route_allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/other/my-svc/health",
		"params": {"namespace": "other", "service": "my-svc", "path": "health"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}

test_svc_spa_allowed if {
	route_allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/mypool/my-svc/health",
		"params": {"namespace": "mypool", "service": "my-svc", "path": "health"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_svc_invalid_namespace_denied if {
	not route_allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/Pool-MyPool/my-svc/health",
		"params": {"namespace": "Pool-MyPool", "service": "my-svc", "path": "health"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "Pool-MyPool", "email": ""},
	}
}

test_svc_invalid_service_denied if {
	not route_allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/mypool/My-Svc/health",
		"params": {"namespace": "mypool", "service": "My-Svc", "path": "health"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}

test_svc_user_key_allowed if {
	route_allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/yup/yup-7766583b-mcpjs/api/exec",
		"params": {"namespace": "yup", "service": "yup-7766583b-mcpjs", "path": "api/exec"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

test_svc_oauth2_proxy_allowed if {
	route_allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/mypool/mypool-abc-mcp/health",
		"params": {"namespace": "mypool", "service": "mypool-abc-mcp", "path": "health"},
		"user": {"sub": "user-123", "azp": "oauth2-proxy", "namespace": "", "email": "u@example.com"},
	}
}

# MCP connector clients (RFC 7591 dynamic registration) — azp is a
# per-registration UUID; the handler's RoleBinding probe against sub is
# the authorization boundary.
test_svc_dcr_client_allowed if {
	route_allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "POST",
		"path": "/api/svc/mypool/mypool-abc-cua-driver/mcp",
		"params": {"namespace": "mypool", "service": "mypool-abc-cua-driver", "path": "mcp"},
		"user": {"sub": "user-123", "azp": "f81d4fae-7dec-11d0-a765-00a0c91e6bf6", "namespace": "", "email": "u@example.com"},
	}
}

test_svc_dcr_client_empty_sub_denied if {
	not route_allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/mypool/my-svc/health",
		"params": {"namespace": "mypool", "service": "my-svc", "path": "health"},
		"user": {"sub": "", "azp": "f81d4fae-7dec-11d0-a765-00a0c91e6bf6", "namespace": "", "email": ""},
	}
}

# The catch-all must NOT relax the per-key namespace binding: a key-*
# client targeting a foreign namespace stays denied even though it is
# authenticated.
test_svc_per_key_not_covered_by_catchall if {
	not route_allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/other/my-svc/health",
		"params": {"namespace": "other", "service": "my-svc", "path": "health"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}
