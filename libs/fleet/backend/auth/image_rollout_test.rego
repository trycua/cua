package image_rollout_test

import rego.v1

import data.image_rollout

image_input(method, sub) := {
	"route": "/api/k8s/{path...}",
	"method": method,
	"params": {"path": "apis/images.cua.ai/v1alpha1/namespaces/tenant-a/images/image-a"},
	"user": {"sub": sub},
	"flags": {"admin_subs": ["admin-owner"]},
}

test_admin_image_mutations_allowed if {
	every method in ["POST", "PATCH", "DELETE"] {
		image_rollout.allow with input as image_input(method, "admin-owner")
	}
}

test_nonadmin_image_mutations_denied if {
	every method in ["POST", "PATCH", "DELETE"] {
		not image_rollout.allow with input as image_input(method, "ordinary-owner")
	}
}

test_read_methods_unchanged if {
	every method in ["GET", "HEAD", "OPTIONS"] {
		image_rollout.allow with input as image_input(method, "ordinary-owner")
	}
}

test_upload_presign_uses_same_subject_membership if {
	image_rollout.allow with input as image_input("POST", "admin-owner")
		with input.route as "/api/image-uploads/presign"
	not image_rollout.allow with input as image_input("POST", "ordinary-owner")
		with input.route as "/api/image-uploads/presign"
}

test_missing_membership_denies if {
	every flags in [{}, {"admin_subs": []}] {
		not image_rollout.allow with input as image_input("POST", "admin-owner")
			with input.flags as flags
	}
}

test_membership_is_subject_not_namespace if {
	not image_rollout.allow with input as image_input("POST", "ordinary-owner")
		with input.flags.admin_subs as ["tenant-a"]
}

test_other_resources_unchanged if {
	every path in [
		"apis/osgym.cua.ai/v1alpha1/namespaces/tenant-a/osgymsandboxclaims",
		"apis/images.cua.ai/v1alpha1/namespaces/tenant-a/builders",
		"apis/other.example/v1alpha1/namespaces/tenant-a/images",
	] {
		every method in ["POST", "PATCH", "DELETE"] {
			image_rollout.allow with input as image_input(method, "ordinary-owner")
				with input.params.path as path
		}
	}
}

test_other_routes_unchanged if {
	every route in ["/api/namespaces", "/api/keys", "/api/user-keys"] {
		image_rollout.allow with input as image_input("POST", "ordinary-owner")
			with input.route as route
	}
}
