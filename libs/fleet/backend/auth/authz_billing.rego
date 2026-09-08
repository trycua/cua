# Stripe-hosted billing browser endpoints: /api/billing/summary,
# /api/billing/setup-session, /api/billing/portal-session.
#
# The backend derives the customer identity from input.user.sub and never
# accepts Stripe ownership identifiers, so this rule authenticates and leaves
# ownership to the handler.
#
# SPA only — not cua-cli, not user keys. These endpoints hand back Stripe-hosted
# URLs meant to be opened in the browser session that asked for them.
#
# The prefix match rather than individual literals is deliberate: a new billing
# browser route is covered the day it is registered. /api/billing/webhook is
# excluded because it is not an authenticated route at all — Stripe signature
# verification is its authentication boundary, and it is registered without the
# authorization stage. The exclusion is belt-and-braces for the day someone moves
# it, and route_policy_correspondence_test.go checks it still names a real route.
package authz_billing

default allow = false

allow {
	startswith(input.route, "/api/billing/")
	input.route != "/api/billing/webhook"
	input.user.azp == "cyclops-cs-spa"
}
