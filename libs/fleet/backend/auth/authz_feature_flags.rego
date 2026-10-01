package authz_feature_flags

import data.authz

allow {
	input.route == "/api/admin/feature-flags"
	authz.is_interactive_client
	input.user.azp == "cyclops-cs-spa"
	authz.is_admin
}

allow {
	input.route == "/api/admin/feature-flags/{key}"
	authz.is_interactive_client
	input.user.azp == "cyclops-cs-spa"
	authz.is_admin
}
