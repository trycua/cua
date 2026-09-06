package pool_admission_test

import data.pool_admission

legacy_path := "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools"

native_path := "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates"

allowed_image := "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:latest"

osworld_v2_digest := "296062593712.dkr.ecr.us-west-2.amazonaws.com/osworld-v2-ubuntu-x86@sha256:6f981825c5970027df510006fcfc1ef7a502d2911f69ed9884f7f217007931dd"

omarchy_digest := "296062593712.dkr.ecr.us-west-2.amazonaws.com/omarchy-workspace@sha256:c9cdba09d8cd2f742b9e9fa3818ca29dbcb66ee40edd057621e2987098226950"

non_admin := {"sub": "user-1"}

non_admin_flags := {"admin_subs": []}

admin := {"sub": "admin-1"}

admin_flags := {"admin_subs": ["admin-1"]}

test_unrelated_request_allowed {
	pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": "api/v1/namespaces/ns-a/configmaps"},
		"body": "not json",
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_malformed_pool_body_denied {
	not pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": legacy_path},
		"body": "not json",
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_no_pull_secret_allowed {
	pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": legacy_path},
		"body": `{"spec":{"template":{"containerDiskImage":"docker.io/library/alpine:3.20"}}}`,
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_non_ecr_secret_denied {
	not pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": legacy_path},
		"body": sprintf(`{"spec":{"template":{"containerDiskImage":%q,"imagePullSecret":"other-secret"}}}`, [allowed_image]),
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_ecr_secret_allowlisted_image_allowed {
	pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": legacy_path},
		"body": sprintf(`{"spec":{"template":{"containerDiskImage":%q,"imagePullSecret":"ecr-credentials"}}}`, [allowed_image]),
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_ecr_secret_osworld_v2_digest_allowed {
	pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": legacy_path},
		"body": sprintf(`{"spec":{"template":{"containerDiskImage":%q,"imagePullSecret":"ecr-credentials"}}}`, [osworld_v2_digest]),
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_ecr_secret_omarchy_digest_allowed {
	pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": native_path},
		"body": sprintf(`{"spec":{"vmTemplate":{"containerDiskImage":%q,"imagePullSecret":"ecr-credentials"}}}`, [omarchy_digest]),
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_ecr_secret_gymdriver_dev_image_allowed {
	pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": legacy_path},
		"body": sprintf(`{"spec":{"template":{"containerDiskImage":%q,"imagePullSecret":"ecr-credentials"}}}`, ["296062593712.dkr.ecr.us-west-2.amazonaws.com/cua-gymdriver-dev:latest"]),
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_repository_prefix_collision_denied {
	not pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": legacy_path},
		"body": `{"spec":{"template":{"containerDiskImage":"296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace-evil:latest","imagePullSecret":"ecr-credentials"}}}`,
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_unrelated_patch_allowed {
	pool_admission.allow with input as {
		"method": "PATCH",
		"params": {"path": legacy_path},
		"body": `{"spec":{"template":{"cpuCores":8}}}`,
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_image_only_patch_denied {
	not pool_admission.allow with input as {
		"method": "PATCH",
		"params": {"path": legacy_path},
		"body": sprintf(`{"spec":{"template":{"containerDiskImage":%q}}}`, [allowed_image]),
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_vm_template_ecr_secret_allowlisted_image_allowed {
	pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": native_path},
		"body": sprintf(`{"spec":{"vmTemplate":{"containerDiskImage":%q,"imagePullSecret":"ecr-credentials"}}}`, [allowed_image]),
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_macos_runtime_denied_for_non_admin {
	not pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": native_path},
		"body": `{"spec":{"vmTemplate":{"runtime":"MacOS"}}}`,
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_macos_runtime_class_denied_for_non_admin {
	not pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": native_path},
		"body": `{"spec":{"vmTemplate":{"runtimeClassName":"cua-macos-native"}}}`,
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_macos_node_selector_denied_for_non_admin {
	not pool_admission.allow with input as {
		"method": "PATCH",
		"params": {"path": sprintf("%s/template-a", [native_path])},
		"body": `{"spec":{"vmTemplate":{"nodeSelector":{"cua.ai/macos":"true"}}}}`,
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_macos_allowed_for_admin {
	pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": native_path},
		"body": `{"spec":{"vmTemplate":{"runtime":"macos"}}}`,
		"user": admin,
		"flags": admin_flags,
	}
}

test_nested_virt_denied_for_non_admin {
	not pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": native_path},
		"body": `{"spec":{"vmTemplate":{"nestedVirtualization":true}}}`,
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_nested_virt_patch_denied_for_non_admin {
	not pool_admission.allow with input as {
		"method": "PATCH",
		"params": {"path": sprintf("%s/template-a", [native_path])},
		"body": `{"spec":{"vmTemplate":{"nestedVirtualization":true}}}`,
		"user": non_admin,
		"flags": non_admin_flags,
	}
}

test_nested_virt_allowed_for_admin {
	pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": native_path},
		"body": `{"spec":{"vmTemplate":{"nestedVirtualization":true}}}`,
		"user": admin,
		"flags": admin_flags,
	}
}

test_nested_virt_false_allowed_for_non_admin {
	pool_admission.allow with input as {
		"method": "POST",
		"params": {"path": native_path},
		"body": `{"spec":{"vmTemplate":{"nestedVirtualization":false}}}`,
		"user": non_admin,
		"flags": non_admin_flags,
	}
}
