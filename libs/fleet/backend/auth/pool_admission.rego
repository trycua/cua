package pool_admission

default allow = false

ecr_pull_secret := "ecr-credentials"

allowed_image_repositories := {
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/cua-server-windows",
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace",
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace-duo",
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/osgym-workspace",
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/osworld-golden-workspace",
	"296062593712.dkr.ecr.us-west-2.amazonaws.com/ubuntu-vnc-cua-kubevirt",
}

template := object.get(object.get(input.object, "spec", {}), "template", {})

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

allow {
	input.method != "PATCH"
	not has_pull_secret
}

allow {
	input.method != "PATCH"
	template.imagePullSecret == ecr_pull_secret
	allowed_image
}

allow {
	input.method == "PATCH"
	not has_pull_secret
	not has_image
}

allow {
	input.method == "PATCH"
	object.get(template, "imagePullSecret", "missing") == null
}

allow {
	input.method == "PATCH"
	template.imagePullSecret == ecr_pull_secret
	allowed_image
}
