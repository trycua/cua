package custom_resource_creation_admission_test

import data.custom_resource_creation_admission

input_for(method, path, enabled, admin_subs, sub, cards, current_year, current_month) := {
	"method": method,
	"params": {"path": path},
	"flags": {
		"admin_subs": admin_subs,
		"require_card_for_custom_resource_creation": enabled,
	},
	"user": {"sub": sub},
	"facts": {
		"stripe_cards": {"cards": cards},
		"time": {
			"current_year": current_year,
			"current_month": current_month,
		},
	},
}

test_disabled_bypasses_custom_resource_create {
	custom_resource_creation_admission.exempt with input as input_for(
		"POST", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", false, [], "user-1", [], 2026, 8,
	)
}

test_admin_bypasses_custom_resource_create {
	custom_resource_creation_admission.exempt with input as input_for(
		"POST", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates", true, ["admin-1"], "admin-1", [], 2026, 8,
	)
}

test_configured_non_admin_sub_bypasses_custom_resource_create {
	custom_resource_creation_admission.exempt with input as {
		"method": "POST",
		"params": {"path": "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools"},
		"flags": {
			"admin_subs": [],
			"card_requirement_exempt_subs": ["exempt-user"],
			"require_card_for_custom_resource_creation": true,
		},
		"user": {"sub": "exempt-user"},
	}
}

test_unconfigured_non_admin_sub_does_not_bypass_custom_resource_create {
	not custom_resource_creation_admission.exempt with input as {
		"method": "POST",
		"params": {"path": "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools"},
		"flags": {
			"admin_subs": [],
			"card_requirement_exempt_subs": ["different-user"],
			"require_card_for_custom_resource_creation": true,
		},
		"user": {"sub": "exempt-user"},
	}
}

test_non_admin_custom_resource_create_not_exempt {
	not custom_resource_creation_admission.exempt with input as input_for(
		"POST", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", true, ["admin-1"], "user-1", [], 2026, 8,
	)
}

test_user_created_before_cutoff_is_grandfathered {
	custom_resource_creation_admission.billing_eligible with input as {
		"facts": {"stripe_cards": {"grandfathered": true, "cards": []}},
	}
}

test_user_created_at_cutoff_without_card_is_not_eligible {
	not custom_resource_creation_admission.billing_eligible with input as {
		"facts": {"stripe_cards": {"grandfathered": false, "cards": []}},
	}
}

test_current_month_card_qualifies {
	custom_resource_creation_admission.has_qualifying_card with input as input_for(
		"POST", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", true, [], "user-1",
		[{"exp_year": 2026, "exp_month": 8}], 2026, 8,
	)
}

test_future_month_same_year_qualifies {
	custom_resource_creation_admission.has_qualifying_card with input as input_for(
		"POST", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", true, [], "user-1",
		[{"exp_year": 2026, "exp_month": 12}], 2026, 8,
	)
}

test_future_year_qualifies {
	custom_resource_creation_admission.has_qualifying_card with input as input_for(
		"POST", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", true, [], "user-1",
		[{"exp_year": 2027, "exp_month": 1}], 2026, 8,
	)
}

test_expired_month_same_year_denies {
	not custom_resource_creation_admission.has_qualifying_card with input as input_for(
		"POST", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", true, [], "user-1",
		[{"exp_year": 2026, "exp_month": 7}], 2026, 8,
	)
}

test_expired_year_denies {
	not custom_resource_creation_admission.has_qualifying_card with input as input_for(
		"POST", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", true, [], "user-1",
		[{"exp_year": 2025, "exp_month": 12}], 2026, 8,
	)
}

test_mixed_list_qualifies {
	custom_resource_creation_admission.has_qualifying_card with input as input_for(
		"POST", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", true, [], "user-1",
		[{"exp_year": 2025, "exp_month": 12}, {"exp_year": 2027, "exp_month": 1}], 2026, 8,
	)
}

test_empty_list_denies {
	not custom_resource_creation_admission.has_qualifying_card with input as input_for(
		"POST", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", true, [], "user-1", [], 2026, 8,
	)
}

test_missing_current_year_denies {
	not custom_resource_creation_admission.has_qualifying_card with input as {
		"facts": {
			"stripe_cards": {"cards": [{"exp_year": 2027, "exp_month": 1}]},
			"time": {"current_month": 8},
		},
	}
}

test_missing_current_month_denies {
	not custom_resource_creation_admission.has_qualifying_card with input as {
		"facts": {
			"stripe_cards": {"cards": [{"exp_year": 2027, "exp_month": 1}]},
			"time": {"current_year": 2026},
		},
	}
}

test_missing_cards_denies {
	not custom_resource_creation_admission.has_qualifying_card with input as {
		"facts": {"time": {"current_year": 2026, "current_month": 8}},
	}
}

test_namespaced_custom_group_collection_is_create {
	custom_resource_creation_admission.is_custom_resource_create with input as input_for(
		"POST", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", true, [], "u", [], 2026, 8,
	)
}

test_namespaced_image_collection_requires_card_admission {
	request := input_for(
		"POST", "apis/images.cua.ai/v1alpha1/namespaces/ns-a/images", true, [], "u", [], 2026, 8,
	)

	custom_resource_creation_admission.is_custom_resource_create with input as request
	not custom_resource_creation_admission.exempt with input as request
	not custom_resource_creation_admission.billing_eligible with input as request
}

test_cluster_custom_group_collection_is_create {
	custom_resource_creation_admission.is_custom_resource_create with input as input_for(
		"POST", "apis/osgym.cua.ai/v1alpha1/osgymsandboxtemplates", true, [], "u", [], 2026, 8,
	)
}

test_named_resource_is_not_create {
	not custom_resource_creation_admission.is_custom_resource_create with input as input_for(
		"POST", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools/pool-a", true, [], "u", [], 2026, 8,
	)
}

test_subresource_is_not_create {
	not custom_resource_creation_admission.is_custom_resource_create with input as input_for(
		"POST", "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates/template-a/status", true, [], "u", [], 2026, 8,
	)
}

test_core_resource_is_not_create {
	not custom_resource_creation_admission.is_custom_resource_create with input as input_for(
		"POST", "api/v1/namespaces/ns-a/configmaps", true, [], "u", [], 2026, 8,
	)
}

test_builtin_api_is_not_create {
	not custom_resource_creation_admission.is_custom_resource_create with input as input_for(
		"POST", "apis/apps/v1/namespaces/ns-a/deployments", true, [], "u", [], 2026, 8,
	)
}

test_crd_definition_is_not_create {
	not custom_resource_creation_admission.is_custom_resource_create with input as input_for(
		"POST", "apis/apiextensions.k8s.io/v1/customresourcedefinitions", true, [], "u", [], 2026, 8,
	)
}

test_lookalike_group_is_not_create {
	not custom_resource_creation_admission.is_custom_resource_create with input as input_for(
		"POST", "apis/evil.cua.ai/v1/namespaces/ns-a/osgymworkspacepools", true, [], "u", [], 2026, 8,
	)
}

test_non_post_is_not_create {
	not custom_resource_creation_admission.is_custom_resource_create with input as input_for(
		"PATCH", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools/pool-a", true, [], "u", [], 2026, 8,
	)
}
