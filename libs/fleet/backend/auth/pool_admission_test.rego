package pool_admission_test

import data.pool_admission

allowed_image := "296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace-duo:latest"

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
