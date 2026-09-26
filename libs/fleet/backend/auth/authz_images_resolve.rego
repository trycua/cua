# /api/images/resolve pins a public registry ref to a digest. It reads public
# registry metadata only (the resolver refuses non-public registries), builds
# and stores nothing, and carries no namespace, so any principal family that
# can hold a tenant grant may call it; there is no ownership boundary to check.
package authz_images_resolve

import data.authz

default allow = false

allow {
	input.route == "/api/images/resolve"
	authz.is_interactive_client
}

allow {
	input.route == "/api/images/resolve"
	input.user.principal_type == "github_oidc"
}

allow {
	input.route == "/api/images/resolve"
	authz.is_user_key_client
}

allow {
	input.route == "/api/images/resolve"
	authz.is_per_key_client
}
