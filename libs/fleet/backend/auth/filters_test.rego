package filters_test

import rego.v1

import data.filters

events := [
	{"reason": "PoolCreated"},
	{"reason": "PoolReady"},
	{"reason": "InternalScaleDecision"},
]

# Admins see every event for the pool.
test_admin_sees_all_events if {
	count(filters.visible_events) == 3 with input as {
		"user": {"sub": "admin-1"},
		"flags": {"admin_subs": ["admin-1"], "event_reason_allowlist": ["PoolCreated"]},
		"events": events,
	}
}

# Non-admins see only events whose reason is in the allowlist.
test_non_admin_sees_allowlisted_only if {
	count(filters.visible_events) == 2 with input as {
		"user": {"sub": "nobody"},
		"flags": {"admin_subs": ["admin-1"], "event_reason_allowlist": ["PoolCreated", "PoolReady"]},
		"events": events,
	}
}

# Empty allowlist → non-admins see nothing.
test_non_admin_empty_allowlist_sees_none if {
	count(filters.visible_events) == 0 with input as {
		"user": {"sub": "nobody"},
		"flags": {"admin_subs": ["admin-1"], "event_reason_allowlist": []},
		"events": events,
	}
}

# Missing flags doc → fail closed (non-admins see nothing).
test_non_admin_missing_flags_sees_none if {
	count(filters.visible_events) == 0 with input as {
		"user": {"sub": "nobody"},
		"events": events,
	}
}
