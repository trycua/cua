# What the Stripe-hosted billing browser routes run:
# All(authz_base.allow, authz_billing.allow).
#
# This is the only surface whose module matches a route prefix rather than
# literals, so the cases below pin both halves of that: the three registered
# billing routes are covered, and the webhook — which is registered without an
# authorization stage at all, Stripe signature verification being its boundary —
# is not.
package authz_billing_test

import rego.v1

import data.authz_base
import data.authz_billing

route_allow if {
	authz_base.allow
	authz_billing.allow
}

billing_request(route, azp, sub) := {
	"route": route,
	"method": "GET",
	"path": route,
	"params": {},
	"user": {"sub": sub, "azp": azp, "namespace": "", "email": "u@example.com"},
}

test_summary_spa_allowed if {
	route_allow with input as billing_request("/api/billing/summary", "cyclops-cs-spa", "user-123")
}

test_setup_session_spa_allowed if {
	route_allow with input as billing_request("/api/billing/setup-session", "cyclops-cs-spa", "user-123")
}

test_portal_session_spa_allowed if {
	route_allow with input as billing_request("/api/billing/portal-session", "cyclops-cs-spa", "user-123")
}

# Not every interactive client: these endpoints hand back Stripe-hosted URLs
# meant for the browser session that asked, so cua-cli is out even though it is
# an exact interactive client everywhere else.
test_cua_cli_denied if {
	not route_allow with input as billing_request("/api/billing/summary", "cua-cli", "user-123")
}

test_user_key_denied if {
	not route_allow with input as billing_request("/api/billing/summary", "ukey-test123abc", "user-123")
}

test_per_key_denied if {
	not route_allow with input as billing_request("/api/billing/summary", "key-foo", "svc-123")
}

test_empty_subject_denied if {
	not route_allow with input as billing_request("/api/billing/summary", "cyclops-cs-spa", "")
}

# The webhook is excluded from the prefix explicitly. It is not registered behind
# the authorization stage, so this rule never runs for it in production — but a
# prefix that swept it in would be a policy claiming to guard a route it does
# not, and route_policy_correspondence_test.go would then report the exclusion as
# the only thing keeping the two honest.
test_webhook_not_covered_by_the_prefix if {
	not route_allow with input as billing_request("/api/billing/webhook", "cyclops-cs-spa", "user-123")
}

# A future billing browser route is covered the day it is registered, which is
# why the module matches a prefix rather than three literals.
test_unregistered_billing_route_covered_by_the_prefix if {
	route_allow with input as billing_request("/api/billing/invoices", "cyclops-cs-spa", "user-123")
}

# The prefix is anchored: a route that merely contains it is not billing.
test_lookalike_prefix_denied if {
	not route_allow with input as billing_request("/api/not-billing/summary", "cyclops-cs-spa", "user-123")
}
