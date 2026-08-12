package pool_admission_test

import data.pool_admission

allowed_image := "296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace-duo:latest"
osworld_v2_digest := "296062593712.dkr.ecr.us-west-2.amazonaws.com/osworld-v2-ubuntu-x86@sha256:6f981825c5970027df510006fcfc1ef7a502d2911f69ed9884f7f217007931dd"

test_no_pull_secret_allowed {
	pool_admission.allow with input as {
		"method": "POST",
		"object": {"spec": {"template": {"containerDiskImage": "docker.io/library/alpine:3.20"}}},
	}
}

test_non_ecr_secret_denied {
	not pool_admission.allow with input as {
		"method": "POST",
		"object": {"spec": {"template": {
			"containerDiskImage": allowed_image,
			"imagePullSecret": "other-secret",
		}}},
	}
}

test_ecr_secret_allowlisted_image_allowed {
	pool_admission.allow with input as {
		"method": "POST",
		"object": {"spec": {"template": {
			"containerDiskImage": allowed_image,
			"imagePullSecret": "ecr-credentials",
		}}},
	}
}

test_ecr_secret_osworld_v2_digest_allowed {
	pool_admission.allow with input as {
		"method": "POST",
		"object": {"spec": {"template": {
			"containerDiskImage": osworld_v2_digest,
			"imagePullSecret": "ecr-credentials",
		}}},
	}
}

test_osworld_v2_repository_prefix_collision_denied {
	not pool_admission.allow with input as {
		"method": "POST",
		"object": {"spec": {"template": {
			"containerDiskImage": "296062593712.dkr.ecr.us-west-2.amazonaws.com/osworld-v2-ubuntu-x86-evil:latest",
			"imagePullSecret": "ecr-credentials",
		}}},
	}
}

test_ecr_secret_disallowed_image_denied {
	not pool_admission.allow with input as {
		"method": "POST",
		"object": {"spec": {"template": {
			"containerDiskImage": "evil.example/workspace:latest",
			"imagePullSecret": "ecr-credentials",
		}}},
	}
}

test_repository_prefix_collision_denied {
	not pool_admission.allow with input as {
		"method": "POST",
		"object": {"spec": {"template": {
			"containerDiskImage": "296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace-evil:latest",
			"imagePullSecret": "ecr-credentials",
		}}},
	}
}

test_unrelated_patch_allowed {
	pool_admission.allow with input as {
		"method": "PATCH",
		"object": {"spec": {"services": [{"name": "server", "targetPort": 8000}]}},
	}
}

test_image_only_patch_denied {
	not pool_admission.allow with input as {
		"method": "PATCH",
		"object": {"spec": {"template": {"containerDiskImage": allowed_image}}},
	}
}

test_secret_only_patch_denied {
	not pool_admission.allow with input as {
		"method": "PATCH",
		"object": {"spec": {"template": {"imagePullSecret": "ecr-credentials"}}},
	}
}

# ── Native OSGymSandboxTemplate shape (spec.vmTemplate) ─────────────────

test_vm_template_no_pull_secret_allowed {
	pool_admission.allow with input as {
		"method": "POST",
		"object": {"spec": {"vmTemplate": {"containerDiskImage": "docker.io/library/alpine:3.20"}}},
	}
}

test_vm_template_non_ecr_secret_denied {
	not pool_admission.allow with input as {
		"method": "POST",
		"object": {"spec": {"vmTemplate": {
			"containerDiskImage": allowed_image,
			"imagePullSecret": "other-secret",
		}}},
	}
}

test_vm_template_ecr_secret_allowlisted_image_allowed {
	pool_admission.allow with input as {
		"method": "POST",
		"object": {"spec": {"vmTemplate": {
			"containerDiskImage": allowed_image,
			"imagePullSecret": "ecr-credentials",
		}}},
	}
}

test_vm_template_ecr_secret_unlisted_image_denied {
	not pool_admission.allow with input as {
		"method": "POST",
		"object": {"spec": {"vmTemplate": {
			"containerDiskImage": "docker.io/evil/image:latest",
			"imagePullSecret": "ecr-credentials",
		}}},
	}
}

test_vm_template_patch_pull_secret_swap_denied {
	not pool_admission.allow with input as {
		"method": "PATCH",
		"object": {"spec": {"vmTemplate": {"imagePullSecret": "other-secret"}}},
	}
}
