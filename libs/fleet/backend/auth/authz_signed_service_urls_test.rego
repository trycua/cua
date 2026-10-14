package authz_signed_service_urls_test

import rego.v1

import data.authz_base
import data.authz_signed_service_urls

route_allow if {
	authz_base.allow
	authz_signed_service_urls.allow
}

test_signed_service_urls_spa_allowed if {
	route_allow with input as {"route": "/api/signed-service-urls/{namespace}", "method": "POST", "path": "/api/signed-service-urls/mypool", "params": {"namespace": "mypool"}, "user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"}}
}

test_signed_service_urls_per_key_matching_namespace_allowed if {
	route_allow with input as {"route": "/api/signed-service-urls/{namespace}", "method": "GET", "path": "/api/signed-service-urls/mypool", "params": {"namespace": "mypool"}, "user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""}}
}

test_signed_service_urls_per_key_wrong_namespace_denied if {
	not route_allow with input as {"route": "/api/signed-service-urls/{namespace}", "method": "GET", "path": "/api/signed-service-urls/other", "params": {"namespace": "other"}, "user": {"sub": "svc-123", "azp": "key-mypool", "namespace": "mypool", "email": ""}}
}

test_signed_service_urls_custom_per_key_prefix_is_namespace_scoped if {
	route_allow with input as {"route": "/api/signed-service-urls/{namespace}", "method": "GET", "path": "/api/signed-service-urls/mypool", "params": {"namespace": "mypool"}, "user": {"sub": "svc-123", "azp": "poolkey-mypool", "key_client_prefix": "poolkey-", "namespace": "mypool", "email": ""}}
	not route_allow with input as {"route": "/api/signed-service-urls/{namespace}", "method": "GET", "path": "/api/signed-service-urls/other", "params": {"namespace": "other"}, "user": {"sub": "svc-123", "azp": "poolkey-mypool", "key_client_prefix": "poolkey-", "namespace": "mypool", "email": ""}}
}

test_signed_service_urls_delete_requires_id if {
	not route_allow with input as {"route": "/api/signed-service-urls/{namespace}/{id}", "method": "DELETE", "path": "/api/signed-service-urls/mypool/", "params": {"namespace": "mypool", "id": ""}, "user": {"sub": "user-123", "azp": "cyclops-cs-spa", "namespace": "", "email": "u@example.com"}}
}
