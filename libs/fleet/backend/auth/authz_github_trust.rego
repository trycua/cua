# /api/github-trust-policies and /api/github-trust-policies/{id} — the trust
# policies that decide which GitHub Actions repositories may mint a principal
# against which namespaces.
#
# Interactive clients, and user keys in their *verified* form only. This is the
# one surface that distinguishes the two: everywhere else a "ukey-" azp is
# enough, but a trust policy grants a repository the ability to act as its
# owner, so the token must also carry the principal_type the middleware stamps
# after resolving that owner.
#
# A github_oidc principal cannot reach this surface at all — a repository must
# not be able to widen the grant it was let in by.
package authz_github_trust

import data.authz

default allow = false

allow {
	input.route == "/api/github-trust-policies"
	authz.is_interactive_client
}

allow {
	input.route == "/api/github-trust-policies/{id}"
	authz.is_interactive_client
}

allow {
	input.route == "/api/github-trust-policies"
	authz.is_verified_user_key_client
}

allow {
	input.route == "/api/github-trust-policies/{id}"
	authz.is_verified_user_key_client
}
