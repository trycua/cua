# Browser-local Bash chat. The backend stores history and proxies LiteLLM, but
# the tool itself runs in the authenticated SPA. Keep service-account and
# per-key credentials off this browser surface.
package authz_chat

import data.authz

default allow = false

allow {
	input.route == "/api/chat/conversations"
	input.user.azp == "cyclops-cs-spa"
}

allow {
	input.route == "/api/chat/conversations/{id}"
	input.user.azp == "cyclops-cs-spa"
}

allow {
	input.route == "/api/chat/conversations/{id}/turns"
	input.user.azp == "cyclops-cs-spa"
}
