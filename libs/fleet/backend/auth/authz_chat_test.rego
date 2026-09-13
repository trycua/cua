package authz_chat_test

import rego.v1

import data.authz_base
import data.authz_chat

route_allow if {
	authz_base.allow
	authz_chat.allow
}

chat_request(route, azp, sub) := {
	"route": route,
	"method": "GET",
	"path": route,
	"params": {},
	"user": {"sub": sub, "azp": azp, "namespace": "", "email": "u@example.com"},
}

test_spa_chat_routes_allowed if {
	every route in [
		"/api/chat/conversations",
		"/api/chat/conversations/{id}",
		"/api/chat/conversations/{id}/turns",
	] {
		route_allow with input as chat_request(route, "cyclops-cs-spa", "user-123")
	}
}

test_non_spa_chat_routes_denied if {
	every azp in ["cua-cli", "ukey-user", "key-pool"] {
		not route_allow with input as chat_request("/api/chat/conversations", azp, "user-123")
	}
}
