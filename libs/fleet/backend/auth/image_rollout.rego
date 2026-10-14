package image_rollout

import data.authz

default allow = false

applies {
	input.route == "/api/image-uploads/presign"
	input.method == "POST"
}

applies {
	input.route == "/api/k8s/{path...}"
	{"POST", "PATCH", "DELETE"}[input.method]
	parts := split(input.params.path, "/")
	count(parts) >= 6
	parts[0] == "apis"
	parts[1] == "images.cua.ai"
	parts[2] == "v1alpha1"
	parts[3] == "namespaces"
	parts[5] == "images"
}

allow {
	not applies
}

allow {
	applies
	authz.is_admin
}
