package authz_ownership

svc_input(user, namespace) = {
	"route": "/api/svc/{namespace}/{service}",
	"method": "GET",
	"params": {"namespace": namespace, "service": "novnc"},
	"user": user,
}

spa_user = {"sub": "u-1", "azp": "cyclops-cs-spa"}

github_user = {
	"sub": "owner-1",
	"azp": "github-oidc",
	"principal_type": "github_oidc",
	"allowed_namespaces": ["ns-a"],
}

per_key_user = {"sub": "svc-1", "azp": "key-ns-a", "namespace": "ns-a"}

# ── GitHub OIDC: the claim is the grant, and no probe is eligible ───────────

test_github_claim_allows {
	claim_allow with input as svc_input(github_user, "ns-a")
}

test_github_claim_denies_other_namespace {
	not claim_allow with input as svc_input(github_user, "ns-b")
}

# The half that keeps a GitHub token off the probe path entirely: even with a
# fact saying "allowed", the conjunction the fact sits behind is closed.
test_github_is_not_probe_eligible {
	not probe_eligible with input as svc_input(github_user, "ns-b")
}

test_github_cannot_be_allowed_by_a_fact {
	not rbac_allow with input as object.union(
		svc_input(github_user, "ns-b"),
		{"facts": {"namespace_rbac": {"allowed": true}}},
	)
}

# ── Per-key clients: bound to their namespace claim, never probed ───────────

test_per_key_claim_allows_its_namespace {
	claim_allow with input as svc_input(per_key_user, "ns-a")
}

test_per_key_claim_denies_other_namespace {
	not claim_allow with input as svc_input(per_key_user, "ns-b")
}

test_per_key_is_not_probe_eligible {
	not probe_eligible with input as svc_input(per_key_user, "ns-b")
}

test_per_key_cannot_be_allowed_by_a_fact {
	not rbac_allow with input as object.union(
		svc_input(per_key_user, "ns-b"),
		{"facts": {"namespace_rbac": {"allowed": true}}},
	)
}

# A per-key token with no namespace claim must not match an empty parameter.
test_per_key_empty_claim_denies {
	not claim_allow with input as svc_input({"sub": "svc-1", "azp": "key-x", "namespace": ""}, "")
}

# ── Everyone else: the fact decides ────────────────────────────────────────

test_spa_has_no_claim {
	not claim_allow with input as svc_input(spa_user, "ns-a")
}

test_spa_is_probe_eligible {
	probe_eligible with input as svc_input(spa_user, "ns-a")
}

test_spa_allowed_by_fact {
	rbac_allow with input as object.union(
		svc_input(spa_user, "ns-a"),
		{"facts": {"namespace_rbac": {"allowed": true}}},
	)
}

test_spa_denied_by_fact {
	not rbac_allow with input as object.union(
		svc_input(spa_user, "ns-a"),
		{"facts": {"namespace_rbac": {"allowed": false}}},
	)
}

# A missing facts document is a deny, not an allow: this is what the leaf
# evaluates to if a future edit drops its WithFacts option.
test_spa_denied_without_facts {
	not rbac_allow with input as svc_input(spa_user, "ns-a")
}

# And so is a fact of the wrong type — `== true` rather than a bare reference,
# so a provider returning a string or a number cannot authorize anything.
test_non_boolean_fact_denies {
	not rbac_allow with input as object.union(
		svc_input(spa_user, "ns-a"),
		{"facts": {"namespace_rbac": {"allowed": "true"}}},
	)
}

# An empty namespace parameter is never probed: the probe path would be
# .../namespaces//rolebindings, which is not a question about anything.
test_empty_namespace_is_not_probe_eligible {
	not probe_eligible with input as svc_input(spa_user, "")
}

# ── Where the boundary applies ─────────────────────────────────────────────

test_namespace_get_applies_and_reads_the_name_param {
	probe_eligible with input as {
		"route": "/api/namespaces/{name}",
		"method": "GET",
		"params": {"name": "ns-a"},
		"user": spa_user,
	}
}

test_namespace_get_denied_by_fact {
	not rbac_allow with input as {
		"route": "/api/namespaces/{name}",
		"method": "GET",
		"params": {"name": "ns-a"},
		"user": spa_user,
		"facts": {"namespace_rbac": {"allowed": false}},
	}
}

# DELETE on the same route never ran this check, so the conjunct has to pass
# unconditionally rather than deny — otherwise moving the check into the policy
# would have quietly locked namespace deletion.
test_namespace_delete_does_not_apply {
	claim_allow with input as {
		"route": "/api/namespaces/{name}",
		"method": "DELETE",
		"params": {"name": "ns-b"},
		"user": spa_user,
	}
}

test_namespace_delete_is_not_probe_eligible {
	not probe_eligible with input as {
		"route": "/api/namespaces/{name}",
		"method": "DELETE",
		"params": {"name": "ns-b"},
		"user": spa_user,
	}
}

# The list route shares the namespaces surface, and names no namespace at all.
test_namespace_list_does_not_apply {
	claim_allow with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"params": {},
		"user": spa_user,
	}
}

test_namespace_list_is_not_probe_eligible {
	not probe_eligible with input as {
		"route": "/api/namespaces",
		"method": "GET",
		"params": {},
		"user": spa_user,
	}
}

# ── target_namespace ───────────────────────────────────────────────────────

test_target_namespace_prefers_the_namespace_param {
	target_namespace == "ns-a" with input as svc_input(spa_user, "ns-a")
}

test_target_namespace_falls_back_to_the_name_param {
	target_namespace == "ns-b" with input as {
		"route": "/api/namespaces/{name}",
		"method": "GET",
		"params": {"name": "ns-b"},
		"user": spa_user,
	}
}

test_target_namespace_empty_param_stays_empty {
	target_namespace == "" with input as svc_input(spa_user, "")
}
