package authz_signed_service_urls

import data.authz

default allow = false

allow {
	authz.is_per_key_client
	valid_params
	input.user.namespace == input.params.namespace
}

allow {
	not authz.is_per_key_client
	not authz.is_legacy_per_key_client
	valid_params
}

valid_params {
	input.route == "/api/signed-service-urls/{namespace}"
	authz.valid_dns_label(input.params.namespace)
}

valid_params {
	input.route == "/api/signed-service-urls/{namespace}/{id}"
	authz.valid_dns_label(input.params.namespace)
	input.params.id != ""
}
