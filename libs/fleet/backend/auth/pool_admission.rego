package pool_admission

import data.authz

default allow = false

ecr_pull_secret := "ecr-credentials"

allowed_image_repositories := {
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/cua-gymdriver-dev",
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/cua-server-windows",
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace",
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace-duo",
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/osgym-workspace",
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/osworld-golden-workspace",
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/osworld-v2-ubuntu-x86",
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/ubuntu-vnc-cua-kubevirt",
}

write_method {
	input.method == "POST"
}

write_method {
	input.method == "PUT"
}

write_method {
	input.method == "PATCH"
}

pool_resource_path {
	parts := split(input.params.path, "/")
	count(parts) == 6
	parts[0] == "apis"
	parts[1] == "osgym.cua.ai"
	parts[2] == "v1alpha1"
	parts[3] == "namespaces"
	parts[4] != ""
	parts[5] == "osgymsandboxtemplates"
}

pool_resource_path {
	parts := split(input.params.path, "/")
	count(parts) == 7
	parts[0] == "apis"
	parts[1] == "osgym.cua.ai"
	parts[2] == "v1alpha1"
	parts[3] == "namespaces"
	parts[4] != ""
	parts[5] == "osgymsandboxtemplates"
	parts[6] != ""
}

pool_resource_path {
	parts := split(input.params.path, "/")
	count(parts) == 6
	parts[0] == "apis"
	parts[1] == "cua.ai"
	parts[2] == "v1"
	parts[3] == "namespaces"
	parts[4] != ""
	parts[5] == "osgymworkspacepools"
}

pool_resource_path {
	parts := split(input.params.path, "/")
	count(parts) == 7
	parts[0] == "apis"
	parts[1] == "cua.ai"
	parts[2] == "v1"
	parts[3] == "namespaces"
	parts[4] != ""
	parts[5] == "osgymworkspacepools"
	parts[6] != ""
}

applies {
	write_method
	pool_resource_path
}

allow {
	not applies
}

request_object := json.unmarshal(input.body) {
	applies
}

spec := object.get(request_object, "spec", {})

vm_template := object.get(spec, "vmTemplate", {})

legacy_template := object.get(spec, "template", {})

template := vm_template {
	vm_template != {}
}

template := legacy_template {
	vm_template == {}
}

has_pull_secret {
	object.get(template, "imagePullSecret", null) != null
}

has_image {
	object.get(template, "containerDiskImage", null) != null
}

allowed_image {
	image := template.containerDiskImage
	repository := allowed_image_repositories[_]
	image == repository
}

allowed_image {
	image := template.containerDiskImage
	repository := allowed_image_repositories[_]
	startswith(image, sprintf("%s:", [repository]))
}

allowed_image {
	image := template.containerDiskImage
	repository := allowed_image_repositories[_]
	startswith(image, sprintf("%s@", [repository]))
}

image_configuration_allowed {
	input.method != "PATCH"
	not has_pull_secret
}

image_configuration_allowed {
	input.method != "PATCH"
	template.imagePullSecret == ecr_pull_secret
	allowed_image
}

image_configuration_allowed {
	input.method == "PATCH"
	not has_pull_secret
	not has_image
}

image_configuration_allowed {
	input.method == "PATCH"
	object.get(template, "imagePullSecret", "missing") == null
}

image_configuration_allowed {
	input.method == "PATCH"
	template.imagePullSecret == ecr_pull_secret
	allowed_image
}

requests_macos {
	lower(object.get(template, "runtime", "")) == "macos"
}

requests_macos {
	object.get(template, "runtimeClassName", "") == "cua-macos-native"
}

requests_macos {
	node_selector := object.get(template, "nodeSelector", {})
	object.get(node_selector, "cua.ai/macos", null) != null
}

macos_allowed {
	not requests_macos
}

macos_allowed {
	requests_macos
	authz.is_admin
}

allow {
	applies
	is_object(request_object)
	image_configuration_allowed
	macos_allowed
}
