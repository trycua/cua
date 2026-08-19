package authz_usage

default allow = false

allow {
	startswith(input.route, "/api/usage/")
	input.method == "GET"
	input.user.azp == "cyclops-cs-spa"
}
