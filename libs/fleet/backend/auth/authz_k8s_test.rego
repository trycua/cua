# What /api/k8s/{path...} runs: All(authz_base.allow, authz_k8s.allow).
#
# route_allow below is that composition, so every case asserts the verdict the
# route produces rather than one module's half of it. Splitting the policy must
# not turn a test of the whole check into a test of a fragment.
#
# route_allow here is base + authz_k8s only. The route also runs
# pool_admission.rego over the request body, which has its own suite in
# pool_admission_test.rego; these cases are about route authorization, which is
# exactly what they were about before the split.
package authz_k8s_test

import rego.v1

import data.authz_base
import data.authz_k8s

route_allow if {
	authz_base.allow
	authz_k8s.allow
}

# Both of these named "api/v1/namespaces" — the bare namespace list — when the
# surface was a denylist and any path it did not exclude was admitted. That
# path is not on the allowlist and nothing observed in production asks for it,
# so these now use a path that is: a namespaced pod list. What they are for is
# unchanged, which is that these two azp values reach the surface at all.
test_k8s_spa_allowed if {
	route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/foo/pods",
		"params": {"path": "api/v1/namespaces/foo/pods"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_k8s_cua_cli_allowed if {
	route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/foo/pods",
		"params": {"path": "api/v1/namespaces/foo/pods"},
		"user": {"sub": "user-123", "azp": "cua-cli", "namespace": "", "email": "u@example.com"},
	}
}

# The bare namespace list itself, which used to be the example of an allowed
# path and is now the example of an unenumerated one.
test_k8s_namespace_list_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces",
		"params": {"path": "api/v1/namespaces"},
		"user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"},
	}
}

test_k8s_nodes_non_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/nodes",
		"params": {"path": "api/v1/nodes"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# Admin used to be an escape hatch over the infra paths. Under an allowlist it
# is not an escape hatch over anything: an allowlist with a bypass is a denylist
# wearing a hat. This is the test that used to assert the opposite.
test_k8s_nodes_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/nodes",
		"params": {"path": "api/v1/nodes"},
		"user": {"sub": "admin-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": ["admin-1"]},
	}
}

test_k8s_cluster_pods_non_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/pods",
		"params": {"path": "api/v1/pods"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_batch_jobs_non_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/batch/v1/namespaces/foo/jobs",
		"params": {"path": "apis/batch/v1/namespaces/foo/jobs"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_cyclops_configmaps_non_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/cyclops-cs/configmaps/batch-configs",
		"params": {"path": "api/v1/namespaces/cyclops-cs/configmaps/batch-configs"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_namespaced_pods_non_admin_allowed if {
	route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/foo/pods",
		"params": {"path": "api/v1/namespaces/foo/pods"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# ── K8s events are denied outright ──────────────────────────────────────────
#
# All eight paths that serve Events, both ends of the collection/item split, and
# every principal family this surface otherwise admits. The eight were read off
# a live apiserver, not guessed: two API groups (core/v1 and events.k8s.io),
# each cluster-scoped and namespaced, each with a legacy /watch/ alias that
# streams the same objects. A deny covering only "api/v1/events" would leave
# seven doors open.

test_k8s_cluster_events_non_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/events",
		"params": {"path": "api/v1/events"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# Admin is the escape hatch over is_infra_k8s_path. It is not one over events:
# the deny sits above the whole disjunction.
test_k8s_cluster_events_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/events",
		"params": {"path": "api/v1/events"},
		"user": {"sub": "admin-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": ["admin-1"]},
	}
}

test_k8s_cluster_event_item_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/events/evt-1",
		"params": {"path": "api/v1/events/evt-1"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_namespaced_events_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/foo/events",
		"params": {"path": "api/v1/namespaces/foo/events"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_namespaced_events_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/foo/events",
		"params": {"path": "api/v1/namespaces/foo/events"},
		"user": {"sub": "admin-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": ["admin-1"]},
	}
}

test_k8s_namespaced_event_item_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/foo/events/evt-1",
		"params": {"path": "api/v1/namespaces/foo/events/evt-1"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# A user API key is the other family that reaches customer-facing paths.
test_user_api_key_k8s_events_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/foo/events",
		"params": {"path": "api/v1/namespaces/foo/events"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# A GitHub OIDC token is granted ns-a, and the grant does not reach events.
# This one holds twice over — github_k8s_request_allowed only ever matches
# apis/<group>/... CRD paths, so it would deny a core/v1 event path anyway. It is
# here so that widening that grant cannot open events by accident.
test_github_k8s_events_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"params": {"path": "api/v1/namespaces/ns-a/events"},
		"user": {
			"sub": "owner-1",
			"principal_type": "github_oidc",
			"allowed_namespaces": ["ns-a"],
		},
		"flags": {"admin_subs": []},
	}
}

# ── The events.k8s.io group serves the same objects ─────────────────────────

test_k8s_group_cluster_events_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/events.k8s.io/v1/events",
		"params": {"path": "apis/events.k8s.io/v1/events"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_group_cluster_events_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/events.k8s.io/v1/events",
		"params": {"path": "apis/events.k8s.io/v1/events"},
		"user": {"sub": "admin-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": ["admin-1"]},
	}
}

test_k8s_group_namespaced_events_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/events.k8s.io/v1/namespaces/foo/events",
		"params": {"path": "apis/events.k8s.io/v1/namespaces/foo/events"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_group_event_item_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/events.k8s.io/v1/namespaces/foo/events/evt-1",
		"params": {"path": "apis/events.k8s.io/v1/namespaces/foo/events/evt-1"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# The version is matched positionally, so a cluster serving the older group
# version is covered without naming it.
test_k8s_group_other_version_events_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/events.k8s.io/v1beta1/events",
		"params": {"path": "apis/events.k8s.io/v1beta1/events"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# ── The legacy /watch/ aliases stream the same objects ──────────────────────
#
# Deprecated since 1.11 and still served. The response is a stream of whole
# Event objects, so a deny that missed these would be a deny in name only.

test_k8s_legacy_watch_cluster_events_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/watch/events",
		"params": {"path": "api/v1/watch/events"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_legacy_watch_namespaced_events_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/watch/namespaces/foo/events",
		"params": {"path": "api/v1/watch/namespaces/foo/events"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_legacy_watch_group_events_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/events.k8s.io/v1/watch/events",
		"params": {"path": "apis/events.k8s.io/v1/watch/events"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_legacy_watch_group_namespaced_events_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/events.k8s.io/v1/watch/namespaces/foo/events",
		"params": {"path": "apis/events.k8s.io/v1/watch/namespaces/foo/events"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# ── Predicate precision ─────────────────────────────────────────────────────
#
# These assert is_event_k8s_path and is_infra_k8s_path directly rather than
# through route_allow, and they have to. Both predicates exist to be *exact*:
# an event path is denied, a path that merely resembles one is not. Under the
# old denylist that distinction was visible in the route verdict, because a
# non-event path fell through to "allowed". Under an allowlist everything
# unenumerated is denied either way, so a route-level test can no longer tell a
# precise predicate from a hopelessly broad one -- both produce a deny.
#
# Asserting the predicate keeps the property under test. A prefix match instead
# of a segment comparison still fails here, which is what the mutation runs
# check.

test_event_predicate_does_not_match_lookalikes if {
	not authz_k8s.is_event_k8s_path("api/v1/eventsources")
	not authz_k8s.is_event_k8s_path("api/v1/namespaces/foo/eventsources")

	# "events" as the namespace, not as the resource.
	not authz_k8s.is_event_k8s_path("api/v1/namespaces/events/pods")

	# A namespaced resource whose name happens to be "events".
	not authz_k8s.is_event_k8s_path("api/v1/namespaces/foo/configmaps/events")

	# A group that merely starts with the events group is a different group.
	not authz_k8s.is_event_k8s_path("apis/events.k8s.io.evil/v1/events")
	not authz_k8s.is_event_k8s_path("apis/notevents.k8s.io/v1/events")

	# A neighbouring resource inside the events group is not the Event resource.
	not authz_k8s.is_event_k8s_path("apis/events.k8s.io/v1/eventsources")

	# A watch on something that is not an event.
	not authz_k8s.is_event_k8s_path("api/v1/watch/namespaces/foo/pods")
}

test_event_predicate_matches_every_served_shape if {
	authz_k8s.is_event_k8s_path("api/v1/events")
	authz_k8s.is_event_k8s_path("api/v1/events/evt-1")
	authz_k8s.is_event_k8s_path("api/v1/namespaces/foo/events")
	authz_k8s.is_event_k8s_path("api/v1/namespaces/foo/events/evt-1")
	authz_k8s.is_event_k8s_path("api/v1/watch/events")
	authz_k8s.is_event_k8s_path("api/v1/watch/namespaces/foo/events")
	authz_k8s.is_event_k8s_path("apis/events.k8s.io/v1/events")
	authz_k8s.is_event_k8s_path("apis/events.k8s.io/v1beta1/events")
	authz_k8s.is_event_k8s_path("apis/events.k8s.io/v1/namespaces/foo/events")
	authz_k8s.is_event_k8s_path("apis/events.k8s.io/v1/watch/events")
	authz_k8s.is_event_k8s_path("apis/events.k8s.io/v1/watch/namespaces/foo/events")
}

# The Capsule group is likewise compared whole, and "tenantowners" is a
# different resource from "tenants".
test_infra_predicate_does_not_match_lookalikes if {
	not authz_k8s.is_infra_k8s_path("apis/capsule.clastix.io/v1beta2/tenantowners")
	not authz_k8s.is_infra_k8s_path("apis/capsule.clastix.io.evil/v1beta2/tenants")
	not authz_k8s.is_infra_k8s_path("api/v1/namespaces/foo/pods")
}

# Cluster-wide CRD lists leak every tenant's pools/claims. Nobody gets them:
# the allowlist admits the namespaced form only.
test_k8s_cluster_pools_non_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/cua.ai/v1/osgymworkspacepools",
		"params": {"path": "apis/cua.ai/v1/osgymworkspacepools"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_cluster_pools_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/cua.ai/v1/osgymworkspacepools",
		"params": {"path": "apis/cua.ai/v1/osgymworkspacepools"},
		"user": {"sub": "admin-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": ["admin-1"]},
	}
}

test_k8s_cluster_claims_non_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims",
		"params": {"path": "apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_capsule_tenants_non_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "POST",
		"path": "/api/k8s/apis/capsule.clastix.io/v1beta2/tenants",
		"params": {"path": "apis/capsule.clastix.io/v1beta2/tenants"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_capsule_tenants_user_key_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "POST",
		"path": "/api/k8s/apis/capsule.clastix.io/v1beta2/tenants",
		"params": {"path": "apis/capsule.clastix.io/v1beta2/tenants"},
		"user": {"sub": "user-1", "azp": "ukey-user-1", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_capsule_tenant_item_non_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/capsule.clastix.io/v1beta2/tenants/user-user-1",
		"params": {"path": "apis/capsule.clastix.io/v1beta2/tenants/user-user-1"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_capsule_tenants_other_versions_non_admin_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "POST",
		"path": "/api/k8s/apis/capsule.clastix.io/v1beta1/tenants",
		"params": {"path": "apis/capsule.clastix.io/v1beta1/tenants"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}

	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/capsule.clastix.io/v99/tenants/user-user-1/status",
		"params": {"path": "apis/capsule.clastix.io/v99/tenants/user-user-1/status"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# These two used to assert that the Capsule neighbours fall through to allowed.
# They are denied now, like everything unenumerated, so the property they were
# guarding -- that is_infra_k8s_path is exact -- moved to
# test_infra_predicate_does_not_match_lookalikes above.
test_k8s_capsule_neighbor_paths_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/capsule.clastix.io/v1beta2/tenantowners",
		"params": {"path": "apis/capsule.clastix.io/v1beta2/tenantowners"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

# Namespaced pool/claim lists are how the SPA reads them — stay open
# (Capsule RBAC scopes the caller to their own namespaces).
test_k8s_namespaced_pools_non_admin_allowed if {
	route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/apis/cua.ai/v1/namespaces/foo/osgymworkspacepools",
		"params": {"path": "apis/cua.ai/v1/namespaces/foo/osgymworkspacepools"},
		"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_k8s_namespaced_claims_non_admin_allowed if {
	route_allow with input as {
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
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/foo/pods",
		"params": {"path": "api/v1/namespaces/foo/pods"},
		"user": {"sub": "admin-1", "azp": "key-foo", "namespace": "foo", "email": ""},
		"flags": {"admin_subs": ["admin-1"]},
	}
}

test_user_api_key_k8s_customer_path_allowed if {
	route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/namespaces/foo/pods",
		"params": {"path": "api/v1/namespaces/foo/pods"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_user_api_key_k8s_infra_path_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"path": "/api/k8s/api/v1/nodes",
		"params": {"path": "api/v1/nodes"},
		"user": {"sub": "user-123", "azp": "ukey-test123abc", "namespace": "", "email": ""},
		"flags": {"admin_subs": []},
	}
}

test_github_k8s_allowed_namespace_collection_get if {
	route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"params": {"path": "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools"},
		"user": {
			"sub": "owner-1",
			"principal_type": "github_oidc",
			"allowed_namespaces": ["ns-a"],
		},
		"flags": {"admin_subs": []},
	}
}

test_github_k8s_outside_namespace_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"params": {"path": "apis/cua.ai/v1/namespaces/ns-b/osgymworkspacepools"},
		"user": {
			"sub": "owner-1",
			"principal_type": "github_oidc",
			"allowed_namespaces": ["ns-a"],
		},
		"flags": {"admin_subs": []},
	}
}

test_github_k8s_template_write_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "POST",
		"params": {"path": "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates"},
		"user": {
			"sub": "owner-1",
			"principal_type": "github_oidc",
			"allowed_namespaces": ["ns-a"],
		},
		"flags": {"admin_subs": []},
	}
}

test_github_k8s_claim_subresource_denied if {
	not route_allow with input as {
		"route": "/api/k8s/{path...}",
		"method": "GET",
		"params": {"path": "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims/claim-a/status"},
		"user": {
			"sub": "owner-1",
			"principal_type": "github_oidc",
			"allowed_namespaces": ["ns-a"],
		},
		"flags": {"admin_subs": []},
	}
}

# ── The allowlist ───────────────────────────────────────────────────────────
#
# One test per admitted block, and then the denials that make it an allowlist
# rather than a longer denylist. `spa` stands for the interactive family
# throughout; test_user_key_gets_the_same_allowlist pins that user keys reach
# the same set.

spa_request(method, path) := {
	"route": "/api/k8s/{path...}",
	"method": method,
	"path": sprintf("/api/k8s/%s", [path]),
	"params": {"path": path},
	"user": {"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""},
	"flags": {"admin_subs": []},
}

# Fleet CRDs, native group: the full matrix, on all three resources.
test_native_fleet_crud_allowed if {
	every resource in ["osgymsandboxclaims", "osgymsandboxwarmpools", "osgymsandboxtemplates"] {
		collection := sprintf("apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/%s", [resource])
		item := sprintf("%s/thing-1", [collection])

		route_allow with input as spa_request("GET", collection)
		route_allow with input as spa_request("POST", collection)
		route_allow with input as spa_request("GET", item)
		route_allow with input as spa_request("PATCH", item)
		route_allow with input as spa_request("DELETE", item)
		route_allow with input as spa_request("PATCH", sprintf("%s/status", [item]))
	}
}

# Legacy pools, including the PUT that production shows succeeding and no rule
# had ever named.
test_legacy_pool_crud_allowed if {
	collection := "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools"
	item := sprintf("%s/pool-1", [collection])

	route_allow with input as spa_request("GET", collection)
	route_allow with input as spa_request("POST", collection)
	route_allow with input as spa_request("GET", item)
	route_allow with input as spa_request("PATCH", item)
	route_allow with input as spa_request("DELETE", item)
	route_allow with input as spa_request("PUT", item)
}

# Core reads: pods, pod logs, services.
test_core_reads_allowed if {
	route_allow with input as spa_request("GET", "api/v1/namespaces/ns-a/pods")
	route_allow with input as spa_request("GET", "api/v1/namespaces/ns-a/pods/pod-1")
	route_allow with input as spa_request("GET", "api/v1/namespaces/ns-a/pods/pod-1/log")
	route_allow with input as spa_request("GET", "api/v1/namespaces/ns-a/services")
	route_allow with input as spa_request("GET", "api/v1/namespaces/ns-a/services/svc-1")
}

test_pod_metrics_allowed if {
	route_allow with input as spa_request("GET", "apis/metrics.k8s.io/v1beta1/namespaces/ns-a/pods")
	route_allow with input as spa_request("GET", "apis/metrics.k8s.io/v1beta1/namespaces/ns-a/pods/pod-1")
}

test_kubevirt_reads_allowed if {
	route_allow with input as spa_request("GET", "apis/kubevirt.io/v1/namespaces/ns-a/virtualmachines")
	route_allow with input as spa_request("GET", "apis/kubevirt.io/v1/namespaces/ns-a/virtualmachineinstances/vmi-1")
}

test_discovery_allowed if {
	route_allow with input as spa_request("GET", "openapi/v2")
	route_allow with input as spa_request("GET", "openapi/v3/apis/cua.ai/v1")
	route_allow with input as spa_request("GET", "apis/cua.ai/v1")
	route_allow with input as spa_request("GET", "apis")
}

test_self_subject_access_review_allowed if {
	route_allow with input as spa_request("POST", "apis/authorization.k8s.io/v1/selfsubjectaccessreviews")
}

test_user_key_gets_the_same_allowlist if {
	request := object.union(
		spa_request("GET", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims"),
		{"user": {"sub": "user-1", "azp": "ukey-abc", "namespace": "", "email": ""}},
	)

	route_allow with input as request
}

# ── Denials: the point of the posture ───────────────────────────────────────
#
# None of these is on any exclusion list. They are denied because nothing
# admits them, which is the whole difference from the denylist this replaced.

test_unenumerated_core_resources_denied if {
	not route_allow with input as spa_request("GET", "api/v1/namespaces/ns-a/secrets")
	not route_allow with input as spa_request("GET", "api/v1/namespaces/ns-a/secrets/ecr-credentials")
	not route_allow with input as spa_request("GET", "api/v1/namespaces/ns-a/configmaps")
	not route_allow with input as spa_request("GET", "api/v1/namespaces/ns-a/serviceaccounts/default")
	not route_allow with input as spa_request("GET", "api/v1/namespaces/ns-a/persistentvolumeclaims")
	not route_allow with input as spa_request("GET", "api/v1/namespaces/ns-a/resourcequotas")
}

# A resource may be readable without being writable. Nothing in traffic or in
# this repo writes a core resource through this proxy.
test_core_writes_denied if {
	not route_allow with input as spa_request("POST", "api/v1/namespaces/ns-a/pods")
	not route_allow with input as spa_request("DELETE", "api/v1/namespaces/ns-a/pods/pod-1")
	not route_allow with input as spa_request("POST", "api/v1/namespaces/ns-a/persistentvolumeclaims")
}

# Sandbox reads are customer-facing, but sandbox writes and unknown verbs stay
# outside the allowlist.
test_unenumerated_resource_and_verb_denied if {
	route_allow with input as spa_request("GET", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes")
	not route_allow with input as spa_request("POST", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes")
	not route_allow with input as spa_request("DELETE", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes/sandbox-1")
	not route_allow with input as spa_request("PUT", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims/claim-1")
}

# An entire API group nobody enumerated. These three all succeeded in
# production as single one-off calls; they are the tail the allowlist
# deliberately drops.
test_unenumerated_groups_denied if {
	not route_allow with input as spa_request("GET", "apis/storage.k8s.io/v1/storageclasses")
	not route_allow with input as spa_request("GET", "apis/external-secrets.io/v1beta1/namespaces/ns-a/externalsecrets")
	not route_allow with input as spa_request("GET", "apis/cdi.kubevirt.io/v1beta1/namespaces/ns-a/datavolumes")
}

# KubeVirt is readable, not drivable.
test_kubevirt_writes_denied if {
	not route_allow with input as spa_request("DELETE", "apis/kubevirt.io/v1/namespaces/ns-a/virtualmachines/vm-1")
	not route_allow with input as spa_request("PUT", "apis/subresources.kubevirt.io/v1/namespaces/ns-a/virtualmachines/vm-1/restart")
}

# The cluster-scoped form of an admitted resource is not admitted: every rule
# above requires the namespaces/{ns} segment.
test_cluster_scoped_forms_denied if {
	not route_allow with input as spa_request("GET", "apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims")
	not route_allow with input as spa_request("GET", "api/v1/pods")
	not route_allow with input as spa_request("GET", "apis/metrics.k8s.io/v1beta1/nodes")
}

# An empty namespace segment addresses the cluster-wide collection on a real
# apiserver, so it must not satisfy a namespaced rule.
test_empty_namespace_segment_denied if {
	not route_allow with input as spa_request("GET", "apis/osgym.cua.ai/v1alpha1/namespaces//osgymsandboxclaims")
	not route_allow with input as spa_request("GET", "api/v1/namespaces//pods")
}

# ── The legacy /watch/ alias agrees with the path it aliases ────────────────
#
# api/v1/watch/pods streams exactly what api/v1/pods lists, so the two must
# answer the same. They did not: is_infra_k8s_path matched the literal spelling
# and the alias walked past it. The bar here is agreement, not "the alias is
# denied" -- a rule that denied every watch path would pass the second and be
# wrong, because namespaced pod watches are customer-facing.

infra_pairs := [
	["api/v1/pods", "api/v1/watch/pods"],
	["api/v1/nodes", "api/v1/watch/nodes"],
	["api/v1/nodes/ip-10-0-0-1", "api/v1/watch/nodes/ip-10-0-0-1"],
	["api/v1/namespaces/cyclops-cs/configmaps", "api/v1/watch/namespaces/cyclops-cs/configmaps"],
	["apis/cua.ai/v1/osgymworkspacepools", "apis/cua.ai/v1/watch/osgymworkspacepools"],
	["apis/capsule.clastix.io/v1beta2/tenants", "apis/capsule.clastix.io/v1beta2/watch/tenants"],
]

# Both spellings are infra.
test_watch_alias_agrees_with_literal_on_infra_paths if {
	every pair in infra_pairs {
		authz_k8s.is_infra_k8s_path(pair[0])
		authz_k8s.is_infra_k8s_path(pair[1])
	}
}

# And both spellings are not, where the literal is not. Namespaced pods are the
# case that matters: a blanket "deny anything with /watch/" would break them.
open_pairs := [
	["api/v1/namespaces/foo/pods", "api/v1/watch/namespaces/foo/pods"],
	["api/v1/namespaces/foo/services", "api/v1/watch/namespaces/foo/services"],
	["apis/osgym.cua.ai/v1alpha1/namespaces/foo/osgymsandboxclaims", "apis/osgym.cua.ai/v1alpha1/watch/namespaces/foo/osgymsandboxclaims"],
]

test_watch_alias_agrees_with_literal_on_open_paths if {
	every pair in open_pairs {
		not authz_k8s.is_infra_k8s_path(pair[0])
		not authz_k8s.is_infra_k8s_path(pair[1])
	}
}

# The rewrite must not fire on a path that merely contains the word. "watch" as
# a namespace name, or as a resource name, is not the alias infix.
test_watch_rewrite_does_not_overreach if {
	not authz_k8s.is_infra_k8s_path("api/v1/namespaces/watch/pods")
	not authz_k8s.is_infra_k8s_path("api/v1/watchdogs")
	not authz_k8s.is_infra_k8s_path("apis/cua.ai/v1/namespaces/foo/watch")

	# The infix has to be "watch" specifically. A rewrite that stripped whatever
	# segment sat in that position would map these onto "api/v1/pods" and
	# "api/v1/nodes" and deny them as infra aliases, which they are not.
	not authz_k8s.is_infra_k8s_path("api/v1/foo/pods")
	not authz_k8s.is_infra_k8s_path("api/v1/namespaces/nodes")
	not authz_k8s.is_infra_k8s_path("apis/cua.ai/v1/proxy/osgymworkspacepools")
}

# At the route level, for every principal family this surface admits: the alias
# and the literal produce the same verdict. Both are denied today -- the alias
# because nothing enumerates it, the literal because nothing enumerates it
# either -- and this pins that they cannot come apart.
test_watch_alias_and_literal_agree_for_every_principal if {
	every azp in ["cyclops-cs-spa", "cua-cli", "ukey-abc"] {
		not route_allow with input as watch_request(azp, "api/v1/pods", false)
		not route_allow with input as watch_request(azp, "api/v1/watch/pods", false)
		not route_allow with input as watch_request(azp, "api/v1/nodes", false)
		not route_allow with input as watch_request(azp, "api/v1/watch/nodes", false)
	}
}

# Admin too. Under the denylist is_admin bypassed the infra list, so the alias
# and the literal could have disagreed for exactly this principal; the allowlist
# removed that bypass and this is what pins it.
test_watch_alias_and_literal_agree_for_admin if {
	not route_allow with input as watch_request("cyclops-cs-spa", "api/v1/pods", true)
	not route_allow with input as watch_request("cyclops-cs-spa", "api/v1/watch/pods", true)
	not route_allow with input as watch_request("cyclops-cs-spa", "api/v1/nodes", true)
	not route_allow with input as watch_request("cyclops-cs-spa", "api/v1/watch/nodes", true)
}

# Namespaced pods stay reachable in both spellings for a normal caller... or
# rather, the literal does; the watch spelling is not on the allowlist, which is
# a separate decision from the infra deny and is recorded as such.
test_namespaced_pod_literal_still_allowed if {
	route_allow with input as watch_request("cyclops-cs-spa", "api/v1/namespaces/foo/pods", false)
}

watch_request(azp, path, admin) := {
	"route": "/api/k8s/{path...}",
	"method": "GET",
	"path": sprintf("/api/k8s/%s", [path]),
	"params": {"path": path},
	"user": {"sub": watch_sub(admin), "azp": azp, "namespace": "", "email": ""},
	"flags": {"admin_subs": watch_admin_subs(admin)},
}

watch_sub(admin) := "admin-1" if {
	admin
}

watch_sub(admin) := "user-1" if {
	not admin
}

watch_admin_subs(admin) := ["admin-1"] if {
	admin
}

watch_admin_subs(admin) := [] if {
	not admin
}

# ── Bound-sandbox service exposure ──────────────────────────────────────────
#
# PATCH on a Sandbox item is the one write clients get on osgymsandboxes, so a
# claim holder can expose an extra service port on a running sandbox by
# appending to spec.vmTemplate.services. The body restriction (only that field
# may move) is sandbox_services_admission.rego's job and is tested there; these
# cases pin the route-authorization half.

test_sandbox_item_patch_allowed if {
	route_allow with input as spa_request("PATCH", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes/sandbox-1")
}

test_sandbox_item_patch_allowed_for_user_key if {
	request := object.union(
		spa_request("PATCH", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes/sandbox-1"),
		{"user": {"sub": "user-1", "azp": "ukey-abc", "namespace": "", "email": ""}},
	)

	route_allow with input as request
}

# The item verb matrix stays otherwise closed: PATCH is the only write, and the
# collection takes no writes at all.
test_sandbox_write_matrix_stays_closed if {
	not route_allow with input as spa_request("PATCH", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes")
	not route_allow with input as spa_request("PUT", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes/sandbox-1")
	not route_allow with input as spa_request("POST", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes")
	not route_allow with input as spa_request("DELETE", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes/sandbox-1")
	not route_allow with input as spa_request("PATCH", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes/sandbox-1/status")
}

# An empty namespace or name segment must not satisfy the item shape.
test_sandbox_patch_needs_namespace_and_name if {
	not route_allow with input as spa_request("PATCH", "apis/osgym.cua.ai/v1alpha1/namespaces//osgymsandboxes/sandbox-1")
	not route_allow with input as spa_request("PATCH", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes/")
}

# GitHub OIDC principals keep their own enumerated grant, which does not
# include sandboxes at all.
test_sandbox_patch_denied_for_github_oidc if {
	request := object.union(
		spa_request("PATCH", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes/sandbox-1"),
		{"user": {
			"sub": "repo:trycua/cloud",
			"azp": "github-oidc",
			"principal_type": "github_oidc",
			"allowed_namespaces": ["ns-a"],
		}},
	)

	not route_allow with input as request
}

# ── The service-write guidance query ────────────────────────────────────────
#
# not_direct_service_write is the Because(...) conjunct K8sRoutePolicy runs
# before the allow leaf: it must deny exactly the core-Services writes (so the
# 403 carries the guidance message) and allow everything else (so it never
# shadows another rule's verdict or reason).

test_direct_service_write_guidance_denies_writes if {
	every method in ["POST", "PUT", "PATCH", "DELETE"] {
		not authz_k8s.not_direct_service_write with input as spa_request(method, "api/v1/namespaces/ns-a/services")
		not authz_k8s.not_direct_service_write with input as spa_request(method, "api/v1/namespaces/ns-a/services/sandbox-1-source-mcp")
	}
}

test_direct_service_write_guidance_passes_everything_else if {
	authz_k8s.not_direct_service_write with input as spa_request("GET", "api/v1/namespaces/ns-a/services")
	authz_k8s.not_direct_service_write with input as spa_request("GET", "api/v1/namespaces/ns-a/services/sandbox-1")
	authz_k8s.not_direct_service_write with input as spa_request("POST", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims")
	authz_k8s.not_direct_service_write with input as spa_request("POST", "api/v1/namespaces/ns-a/pods")
	authz_k8s.not_direct_service_write with input as spa_request("PATCH", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes/sandbox-1")
}

# The denial the guidance annotates is real with or without the annotation:
# the allowlist itself never admits a core-Services write. This is the exact
# request shape from the field report — POST a ClusterIP Service selecting the
# sandbox — and it must stay denied by both conjuncts.
test_direct_service_create_denied if {
	not route_allow with input as spa_request("POST", "api/v1/namespaces/ns-a/services")
}
