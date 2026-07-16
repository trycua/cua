package authz_test

import rego.v1

import data.authz

# ── /api/keys ────────────────────────────────────────────────────────────────

test_keys_spa_allowed if {
	authz.allow with input as {
		"route": "/api/keys",
		"method": "POST",
		"path": "/api/keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_keys_non_spa_denied if {
	not authz.allow with input as {
		"route": "/api/keys",
		"method": "POST",
		"path": "/api/keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "key-some-pool", "namespace": "some-pool", "email": "u@example.com"},
	}
}

# ── /api/gateway ──────────────────────────────────────────────────────────────
#
# Policy (CUA-527): token's `namespace` claim MUST equal the pool name.
# A per-key client holds exactly one pool; namespace is set by the Keycloak
# hardcoded-claim mapper.

test_gateway_exact_namespace_allowed if {
	authz.allow with input as {
		"route": "/api/gateway/{name}",
		"method": "GET",
		"path": "/api/gateway/mypool",
		"params": {"name": "mypool"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}

test_gateway_path_exact_namespace_allowed if {
	authz.allow with input as {
		"route": "/api/gateway/{name}/{path...}",
		"method": "GET",
		"path": "/api/gateway/mypool/step",
		"params": {"name": "mypool", "path": "step"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}

# Wrong namespace — must be rejected (cross-pool attack).
test_gateway_wrong_namespace_denied if {
	not authz.allow with input as {
		"route": "/api/gateway/{name}",
		"method": "GET",
		"path": "/api/gateway/mypool",
		"params": {"name": "mypool"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "otherpool", "email": ""},
	}
}

# Namespace matches pool name — allowed (1:1 mapping).
test_gateway_matching_name_namespace_allowed if {
	authz.allow with input as {
		"route": "/api/gateway/{name}",
		"method": "GET",
		"path": "/api/gateway/mypool",
		"params": {"name": "mypool"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}

# Empty namespace — denied.
test_gateway_empty_namespace_denied if {
	not authz.allow with input as {
		"route": "/api/gateway/{name}",
		"method": "GET",
		"path": "/api/gateway/mypool",
		"params": {"name": "mypool"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "", "email": ""},
	}
}

# SPA token on gateway route — denied (must use per-key client).
test_gateway_spa_token_denied if {
	not authz.allow with input as {
		"route": "/api/gateway/{name}",
		"method": "GET",
		"path": "/api/gateway/mypool",
		"params": {"name": "mypool"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "mypool", "email": "u@example.com"},
	}
}

# Invalid pool name (contains uppercase) — denied by DNS-label check.
test_gateway_invalid_pool_name_denied if {
	not authz.allow with input as {
		"route": "/api/gateway/{name}",
		"method": "GET",
		"path": "/api/gateway/MyPool",
		"params": {"name": "MyPool"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "MyPool", "email": ""},
	}
}

# ── /api/k8s ──────────────────────────────────────────────────────────────────

test_k8s_spa_allowed if {
	authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces",
		"params": {"path": "api/v1/namespaces"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

# ── /api/orch ─────────────────────────────────────────────────────────────────

test_orch_spa_allowed if {
	authz.allow with input as {
		"route": "/api/orch/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/orch/mypool/mypool-orchestrator/status",
		"params": {"namespace": "mypool", "service": "mypool-orchestrator", "path": "status"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

# ── /api/batch and /api/label ────────────────────────────────────────────────
#
# Pool is in the URL path. A per-key token's `namespace` claim MUST equal
# the pool name, mirroring /api/gateway/{name}. SPA tokens can target any
# pool.

test_batch_submit_spa_allowed if {
	authz.allow with input as {
		"route": "/api/batch/{pool}/submit",
		"method": "POST",
		"path": "/api/batch/foo/submit",
		"params": {"pool": "foo"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_batch_submit_spa_allowed_any_pool if {
	authz.allow with input as {
		"route": "/api/batch/{pool}/submit",
		"method": "POST",
		"path": "/api/batch/bar/submit",
		"params": {"pool": "bar"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_batch_submit_per_key_matching_namespace_allowed if {
	authz.allow with input as {
		"route": "/api/batch/{pool}/submit",
		"method": "POST",
		"path": "/api/batch/foo/submit",
		"params": {"pool": "foo"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

test_batch_submit_per_key_mismatched_namespace_denied if {
	not authz.allow with input as {
		"route": "/api/batch/{pool}/submit",
		"method": "POST",
		"path": "/api/batch/bar/submit",
		"params": {"pool": "bar"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

test_batch_submit_per_key_empty_namespace_denied if {
	not authz.allow with input as {
		"route": "/api/batch/{pool}/submit",
		"method": "POST",
		"path": "/api/batch/foo/submit",
		"params": {"pool": "foo"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "", "email": ""},
	}
}

test_batch_submit_invalid_pool_denied if {
	not authz.allow with input as {
		"route": "/api/batch/{pool}/submit",
		"method": "POST",
		"path": "/api/batch/Foo/submit",
		"params": {"pool": "Foo"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_batch_lanes_per_key_matching_namespace_allowed if {
	authz.allow with input as {
		"route": "/api/batch/{pool}/lanes",
		"method": "POST",
		"path": "/api/batch/foo/lanes",
		"params": {"pool": "foo"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

test_batch_lanes_per_key_cross_pool_denied if {
	not authz.allow with input as {
		"route": "/api/batch/{pool}/lanes",
		"method": "DELETE",
		"path": "/api/batch/bar/lanes",
		"params": {"pool": "bar"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

test_batch_status_per_key_matching_namespace_allowed if {
	authz.allow with input as {
		"route": "/api/batch/{pool}/{id}/status",
		"method": "GET",
		"path": "/api/batch/foo/abc-123/status",
		"params": {"pool": "foo", "id": "abc-123"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

test_batch_status_per_key_cross_pool_denied if {
	not authz.allow with input as {
		"route": "/api/batch/{pool}/{id}/status",
		"method": "GET",
		"path": "/api/batch/bar/abc-123/status",
		"params": {"pool": "bar", "id": "abc-123"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

test_label_submit_per_key_matching_namespace_allowed if {
	authz.allow with input as {
		"route": "/api/label/{pool}/{label}/batch",
		"method": "POST",
		"path": "/api/label/foo/run-1/batch",
		"params": {"pool": "foo", "label": "run-1"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

test_label_submit_per_key_cross_pool_denied if {
	not authz.allow with input as {
		"route": "/api/label/{pool}/{label}/batch",
		"method": "POST",
		"path": "/api/label/bar/run-1/batch",
		"params": {"pool": "bar", "label": "run-1"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

test_label_status_spa_allowed if {
	authz.allow with input as {
		"route": "/api/label/{pool}/{label}/status",
		"method": "GET",
		"path": "/api/label/foo/run-1/status",
		"params": {"pool": "foo", "label": "run-1"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

# ── /api/svc — generic per-key service proxy ─────────────────────────────────

test_svc_per_key_matching_namespace_allowed if {
	authz.allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/mypool/mypool-orchestrator/status",
		"params": {"namespace": "mypool", "service": "mypool-orchestrator", "path": "status"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}

test_svc_per_key_no_trailing_path_allowed if {
	authz.allow with input as {
		"route": "/api/svc/{namespace}/{service}",
		"method": "GET",
		"path": "/api/svc/mypool/my-svc",
		"params": {"namespace": "mypool", "service": "my-svc"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}

test_svc_per_key_wrong_namespace_denied if {
	not authz.allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/other/my-svc/health",
		"params": {"namespace": "other", "service": "my-svc", "path": "health"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}

test_svc_spa_allowed if {
	authz.allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/mypool/my-svc/health",
		"params": {"namespace": "mypool", "service": "my-svc", "path": "health"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_svc_invalid_namespace_denied if {
	not authz.allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/Pool-MyPool/my-svc/health",
		"params": {"namespace": "Pool-MyPool", "service": "my-svc", "path": "health"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "Pool-MyPool", "email": ""},
	}
}

test_svc_invalid_service_denied if {
	not authz.allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/mypool/My-Svc/health",
		"params": {"namespace": "mypool", "service": "My-Svc", "path": "health"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}

test_svc_user_key_allowed if {
	authz.allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/yup/yup-7766583b-mcpjs/api/exec",
		"params": {"namespace": "yup", "service": "yup-7766583b-mcpjs", "path": "api/exec"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

test_svc_oauth2_proxy_allowed if {
	authz.allow with input as {
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
	authz.allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "POST",
		"path": "/api/svc/mypool/mypool-abc-cua-driver/mcp",
		"params": {"namespace": "mypool", "service": "mypool-abc-cua-driver", "path": "mcp"},
		"user": {"sub": "user-123", "azp": "f81d4fae-7dec-11d0-a765-00a0c91e6bf6", "namespace": "", "email": "u@example.com"},
	}
}

test_svc_dcr_client_empty_sub_denied if {
	not authz.allow with input as {
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
	not authz.allow with input as {
		"route": "/api/svc/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/svc/other/my-svc/health",
		"params": {"namespace": "other", "service": "my-svc", "path": "health"},
		"user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""},
	}
}

# ── CUA-515: admin flag + infra-path gating on /api/k8s ───────────────────────
#
# Admin membership comes from input.flags.admin_subs (resolved per-request,
# TTL-cached). Infra-only kubectl-proxy paths require admin; customer-facing
# paths stay open to any SPA user.

test_is_admin_true if {
	authz.is_admin with input as {
		"user": {"sub": "admin-1"},
		"flags": {"admin_subs": ["admin-1", "admin-2"]},
	}
}

test_is_admin_false_not_listed if {
	not authz.is_admin with input as {
		"user": {"sub": "nobody"},
		"flags": {"admin_subs": ["admin-1"]},
	}
}

test_is_admin_false_no_flags if {
	not authz.is_admin with input as {"user": {"sub": "admin-1"}}
}

# Infra-only paths — denied for non-admins, allowed for admins.

test_k8s_nodes_non_admin_denied if {
	not authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/nodes",
		"params": {"path": "api/v1/nodes"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_nodes_admin_allowed if {
	authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/nodes",
		"params": {"path": "api/v1/nodes"},
		"user": {"sub": "admin-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": ["admin-1"]},
	}
}

test_k8s_cluster_pods_non_admin_denied if {
	not authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/pods",
		"params": {"path": "api/v1/pods"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_batch_jobs_non_admin_denied if {
	not authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/batch/v1/namespaces/foo/jobs",
		"params": {"path": "apis/batch/v1/namespaces/foo/jobs"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_cyclops_configmaps_non_admin_denied if {
	not authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/cyclops-cs/configmaps/batch-configs",
		"params": {"path": "api/v1/namespaces/cyclops-cs/configmaps/batch-configs"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# Customer-facing paths — allowed for non-admins (event contents are still
# redacted downstream by filters.visible_events).

test_k8s_namespaced_pods_non_admin_allowed if {
	authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/foo/pods",
		"params": {"path": "api/v1/namespaces/foo/pods"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_events_non_admin_allowed if {
	authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/events",
		"params": {"path": "api/v1/events"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# Cluster-wide CRD lists leak every tenant's pools/claims — admin only.
test_k8s_cluster_pools_non_admin_denied if {
	not authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/cua.ai/v1/osgymworkspacepools",
		"params": {"path": "apis/cua.ai/v1/osgymworkspacepools"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_cluster_pools_admin_allowed if {
	authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/cua.ai/v1/osgymworkspacepools",
		"params": {"path": "apis/cua.ai/v1/osgymworkspacepools"},
		"user": {"sub": "admin-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": ["admin-1"]},
	}
}

test_k8s_cluster_claims_non_admin_denied if {
	not authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims",
		"params": {"path": "apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# Namespaced pool/claim lists are how the SPA reads them — stay open
# (Capsule RBAC scopes the caller to their own namespaces).
test_k8s_namespaced_pools_non_admin_allowed if {
	authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/cua.ai/v1/namespaces/foo/osgymworkspacepools",
		"params": {"path": "apis/cua.ai/v1/namespaces/foo/osgymworkspacepools"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_namespaced_claims_non_admin_allowed if {
	authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/foo/osgymsandboxclaims",
		"params": {"path": "apis/osgym.cua.ai/v1alpha1/namespaces/foo/osgymsandboxclaims"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# Non-SPA tokens are denied on /api/k8s even if listed as admin.
test_k8s_non_spa_denied if {
	not authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/foo/pods",
		"params": {"path": "api/v1/namespaces/foo/pods"},
		"user": {"sub": "admin-1", "azp": "key-foo", "namespace": "foo", "email": ""},
		"flags": {"admin_subs": ["admin-1"]},
	}
}

# ── /api/namespaces — namespace management ──────────────────────────────────

test_namespaces_spa_list_allowed if {
	authz.allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"path": "/api/namespaces",
		"params": {},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_namespaces_spa_create_allowed if {
	authz.allow with input as {
		"route": "/api/namespaces/{name}",
		"method": "POST",
		"path": "/api/namespaces/my-ns",
		"params": {"name": "my-ns"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_namespaces_spa_delete_allowed if {
	authz.allow with input as {
		"route": "/api/namespaces/{name}",
		"method": "DELETE",
		"path": "/api/namespaces/my-ns",
		"params": {"name": "my-ns"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

# Per-key clients (key-*) CANNOT access namespace routes.
test_namespaces_per_key_list_denied if {
	not authz.allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"path": "/api/namespaces",
		"params": {},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

test_namespaces_per_key_by_name_denied if {
	not authz.allow with input as {
		"route": "/api/namespaces/{name}",
		"method": "DELETE",
		"path": "/api/namespaces/my-ns",
		"params": {"name": "my-ns"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

# Empty sub denied on namespace routes.
test_namespaces_empty_sub_denied if {
	not authz.allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"path": "/api/namespaces",
		"params": {},
		"user": {"sub": "", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

# Missing azp denied on namespace routes.
test_namespaces_missing_azp_denied if {
	not authz.allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"path": "/api/namespaces",
		"params": {},
		"user": {"sub": "user-123", "azp": "", "namespace": "", "email": "u@example.com"},
	}
}

# ── /api/pool-templates — per-user saved pool configs ───────────────────────

test_pool_templates_spa_list_allowed if {
	authz.allow with input as {
		"route": "/api/pool-templates",
		"method": "GET",
		"path": "/api/pool-templates",
		"params": {},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_pool_templates_spa_create_allowed if {
	authz.allow with input as {
		"route": "/api/pool-templates",
		"method": "POST",
		"path": "/api/pool-templates",
		"params": {},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_pool_templates_spa_get_allowed if {
	authz.allow with input as {
		"route": "/api/pool-templates/{name}",
		"method": "GET",
		"path": "/api/pool-templates/gpu-large",
		"params": {"name": "gpu-large"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_pool_templates_spa_delete_allowed if {
	authz.allow with input as {
		"route": "/api/pool-templates/{name}",
		"method": "DELETE",
		"path": "/api/pool-templates/gpu-large",
		"params": {"name": "gpu-large"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

# Per-key clients (key-*) CANNOT touch pool templates — those tokens are
# bound to a single pool, not a user identity.
test_pool_templates_per_key_denied if {
	not authz.allow with input as {
		"route": "/api/pool-templates",
		"method": "GET",
		"path": "/api/pool-templates",
		"params": {},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

# User API keys (ukey-*) act on behalf of a user and may manage templates.
test_pool_templates_user_key_list_allowed if {
	authz.allow with input as {
		"route": "/api/pool-templates",
		"method": "GET",
		"path": "/api/pool-templates",
		"params": {},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

test_pool_templates_user_key_delete_allowed if {
	authz.allow with input as {
		"route": "/api/pool-templates/{name}",
		"method": "DELETE",
		"path": "/api/pool-templates/gpu-large",
		"params": {"name": "gpu-large"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

# Empty sub denied on pool-template routes.
test_pool_templates_empty_sub_denied if {
	not authz.allow with input as {
		"route": "/api/pool-templates",
		"method": "GET",
		"path": "/api/pool-templates",
		"params": {},
		"user": {"sub": "", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

# Missing azp denied on pool-template routes.
test_pool_templates_missing_azp_denied if {
	not authz.allow with input as {
		"route": "/api/pool-templates",
		"method": "GET",
		"path": "/api/pool-templates",
		"params": {},
		"user": {"sub": "user-123", "azp": "", "namespace": "", "email": "u@example.com"},
	}
}

# ── /api/user-keys — per-user API key management ────────────────────────────

test_user_keys_spa_create_allowed if {
	authz.allow with input as {
		"route": "/api/user-keys",
		"method": "POST",
		"path": "/api/user-keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_user_keys_spa_list_allowed if {
	authz.allow with input as {
		"route": "/api/user-keys",
		"method": "GET",
		"path": "/api/user-keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_user_keys_spa_delete_allowed if {
	authz.allow with input as {
		"route": "/api/user-keys/{id}",
		"method": "DELETE",
		"path": "/api/user-keys/abc123",
		"params": {"id": "abc123"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

# User-key clients cannot manage other user API keys (must use SPA).
test_user_keys_api_key_denied if {
	not authz.allow with input as {
		"route": "/api/user-keys",
		"method": "GET",
		"path": "/api/user-keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

# Per-key clients cannot manage user API keys.
test_user_keys_per_key_denied if {
	not authz.allow with input as {
		"route": "/api/user-keys",
		"method": "POST",
		"path": "/api/user-keys",
		"params": {},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

# Empty sub denied on user-key management routes.
test_user_keys_empty_sub_denied if {
	not authz.allow with input as {
		"route": "/api/user-keys",
		"method": "GET",
		"path": "/api/user-keys",
		"params": {},
		"user": {"sub": "", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
	}
}

# Missing azp denied on user-key management routes.
test_user_keys_missing_azp_denied if {
	not authz.allow with input as {
		"route": "/api/user-keys",
		"method": "POST",
		"path": "/api/user-keys",
		"params": {},
		"user": {"sub": "user-123", "azp": "", "namespace": "", "email": "u@example.com"},
	}
}

# Per-key client cannot delete user API keys.
test_user_keys_per_key_delete_denied if {
	not authz.allow with input as {
		"route": "/api/user-keys/{id}",
		"method": "DELETE",
		"path": "/api/user-keys/abc123",
		"params": {"id": "abc123"},
		"user": {"sub": "svc-123", "azp": "key-foo", "namespace": "foo", "email": ""},
	}
}

# User-key client cannot delete user API keys (only SPA).
test_user_keys_user_key_delete_denied if {
	not authz.allow with input as {
		"route": "/api/user-keys/{id}",
		"method": "DELETE",
		"path": "/api/user-keys/abc123",
		"params": {"id": "abc123"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

# ── User-key client (azp starts with "ukey-") access to SPA surfaces ────────

test_user_api_key_namespaces_allowed if {
	authz.allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"path": "/api/namespaces",
		"params": {},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

test_user_api_key_namespaces_delete_allowed if {
	authz.allow with input as {
		"route": "/api/namespaces/{name}",
		"method": "DELETE",
		"path": "/api/namespaces/my-ns",
		"params": {"name": "my-ns"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

test_user_api_key_k8s_customer_path_allowed if {
	authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/foo/pods",
		"params": {"path": "api/v1/namespaces/foo/pods"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_user_api_key_k8s_infra_path_denied if {
	not authz.allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/nodes",
		"params": {"path": "api/v1/nodes"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_user_api_key_orch_allowed if {
	authz.allow with input as {
		"route": "/api/orch/{namespace}/{service}/{path...}",
		"method": "GET",
		"path": "/api/orch/mypool/mypool-orchestrator/status",
		"params": {"namespace": "mypool", "service": "mypool-orchestrator", "path": "status"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

test_user_api_key_batch_allowed if {
	authz.allow with input as {
		"route": "/api/batch/{pool}/submit",
		"method": "POST",
		"path": "/api/batch/foo/submit",
		"params": {"pool": "foo"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

test_user_api_key_config_allowed if {
	authz.allow with input as {
		"route": "/api/config",
		"method": "GET",
		"path": "/api/config",
		"params": {},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}

# User-key clients with empty sub are denied.
test_user_api_key_no_sub_denied if {
	not authz.allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"path": "/api/namespaces",
		"params": {},
		"user": {"sub": "", "azp": "ukey-test123abc", "namespace": "", "email": ""},
	}
}
