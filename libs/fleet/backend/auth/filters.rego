# Cyclops-CS data-visibility filters.
#
# This file is intentionally separate from authz.rego.  authz.rego decides
# whether a request is *allowed*; this file decides *what data* an allowed
# request may see.  The two concerns are kept apart so each can evolve,
# be tested, and be reasoned about independently.
#
# Currently covers one endpoint: /api/k8s (kubectl-proxy passthrough).
# The K8s event-filter middleware calls visible_events after receiving the
# upstream EventList and before forwarding it to the SPA, passing the event
# items as part of `input`.
#
# Flags arrive via input.flags (resolved per-request from the OpenFeature
# provider and TTL-cached in auth/middlewares.go — NOT a static OPA data
# store):
#   admin_subs             — Keycloak sub UUIDs with admin data access
#   event_reason_allowlist — K8s event reasons visible to non-admins
#
# Admin membership (input.flags.admin_subs) is decided by data.authz.is_admin;
# this package reuses it rather than redefining the rule.
package filters

# ── K8s EventList filter ────────────────────────────────────────────────────
#
# visible_events filters the K8s EventList items array returned by the
# kubectl-proxy before the SPA receives it.
#
# Input fields used:
#   input.user.sub — caller's JWT subject
#   input.flags    — resolved feature flags (admin_subs, event_reason_allowlist)
#   input.events   — []EventObject from the upstream EventList.items
#
# Admins: all events pass through.
# Non-admins: only events whose `reason` field is in
#             input.flags.event_reason_allowlist.

visible_events[event] {
    event := input.events[_]
    data.authz.is_admin
}

visible_events[event] {
    event := input.events[_]
    not data.authz.is_admin
    input.flags.event_reason_allowlist[_] == event.reason
}
