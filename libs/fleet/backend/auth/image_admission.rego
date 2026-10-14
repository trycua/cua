package image_admission

default allow = false

applies {
	input.route == "/api/k8s/{path...}"
	{"POST", "PUT", "PATCH"}[input.method]
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
	write_format_allowed
	request := json.unmarshal(input.body)
	is_object(request)
	object.keys(request) - {"apiVersion", "kind", "metadata", "spec"} == set()
	object.get(request, "apiVersion", "images.cua.ai/v1alpha1") == "images.cua.ai/v1alpha1"
	object.get(request, "kind", "Image") == "Image"
	metadata := object.get(request, "metadata", {})
	is_object(metadata)
	object.keys(metadata) - {"name", "namespace", "resourceVersion"} == set()
	namespace := split(input.params.path, "/")[4]
	object.get(metadata, "namespace", namespace) == namespace
	is_object(object.get(request, "spec", {}))
}

write_format_allowed {
	input.method == "POST"
}

write_format_allowed {
	input.method == "PATCH"
	input.content_types == ["application/merge-patch+json"]
}
