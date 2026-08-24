# /api/image-uploads/presign signs writes only after the handler proves the
# payload namespace belongs to the authenticated subject. This surface limits
# the route to the same principal families that can hold a tenant grant; the
# handler is deliberately the ownership boundary because the namespace is JSON
# payload data rather than a route parameter available to authz_ownership.rego.
package authz_image_uploads

import data.authz

default allow = false

allow {
	input.route == "/api/image-uploads/presign"
	authz.is_interactive_client
}

allow {
	input.route == "/api/image-uploads/presign"
	input.user.principal_type == "github_oidc"
}

allow {
	input.route == "/api/image-uploads/presign"
	authz.is_user_key_client
}

allow {
	input.route == "/api/image-uploads/presign"
	authz.is_per_key_client
}
