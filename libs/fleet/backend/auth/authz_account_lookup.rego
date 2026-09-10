package authz_account_lookup

import data.authz

allow {
	input.route == "/api/admin/account-lookup"
	input.method == "POST"
	input.user.azp == "cyclops-cs-spa"
	input.user.principal_type == "user"
	authz.is_admin
}
