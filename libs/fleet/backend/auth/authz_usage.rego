package authz_usage

default allow = false

allow {
	input.route == "/api/usage/overview"
	input.method == "GET"
	input.user.azp == "cyclops-cs-spa"
}

allow {
	input.route == "/api/usage/pool"
	input.method == "GET"
	input.user.azp == "cyclops-cs-spa"
}

allow {
	input.route == "/api/usage/browser-timings"
	input.method == "POST"
	input.user.azp == "cyclops-cs-spa"
}
