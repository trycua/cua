# The shared principal vocabulary: is_admin, the token-family predicates, and the
# DNS-label shape check. No route names appear here, because authz.rego holds no
# allow rule — what a principal may do on a surface is that surface's test file.
package authz_test

import rego.v1

import data.authz

test_is_admin_true if {
	authz.is_admin with input as {
		"user": {"sub": "admin-1"},
		"flags": {"admin_subs": ["admin-1", "admin-2"]},
	}
}

test_is_admin_false_not_listed if {
	not authz.is_admin with input as {
		"user": {"sub": "nobody"},
		"flags": {"admin_subs": ["admin-1"]},
	}
}

test_is_admin_false_no_flags if {
	not authz.is_admin with input as {"user": {"sub": "admin-1"}}
}

# ── Token families ──────────────────────────────────────────────────────────
#
# These predicates decided route access from inside one module before the split;
# now every surface imports them, so a change here reaches ten modules at
# once. That is the point of the shared vocabulary, and the reason it is worth
# testing directly rather than only through the surfaces that happen to use it.

test_interactive_clients_are_exactly_two if {
	authz.is_interactive_client with input as {"user": {"azp": "cyclops-cs-spa"}}
	authz.is_interactive_client with input as {"user": {"azp": "cua-cli"}}
	not authz.is_interactive_client with input as {"user": {"azp": "kubernetes"}}
	not authz.is_interactive_client with input as {"user": {"azp": ""}}
}

test_per_key_client_recognised_by_prefix if {
	authz.is_per_key_client with input as {"user": {"azp": "key-foo"}}
	not authz.is_per_key_client with input as {"user": {"azp": "cyclops-cs-spa"}}
	not authz.is_per_key_client with input as {"user": {"azp": "ukey-foo"}}
}

test_per_key_client_uses_configured_prefix if {
	authz.is_per_key_client with input as {"user": {"azp": "poolkey-foo", "key_client_prefix": "poolkey-"}}
	not authz.is_per_key_client with input as {"user": {"azp": "key-foo", "key_client_prefix": "poolkey-"}}
}

test_user_key_client_recognised_by_prefix if {
	authz.is_user_key_client with input as {"user": {"azp": "ukey-foo"}}
	not authz.is_user_key_client with input as {"user": {"azp": "key-foo"}}
}

# The verified form needs the principal_type the middleware stamps only after it
# has resolved the key's owner. An azp that merely looks like a user key is not
# enough — /api/github-trust-policies is the surface that relies on this.
test_verified_user_key_needs_the_resolved_principal_type if {
	authz.is_verified_user_key_client with input as {"user": {"azp": "ukey-foo", "principal_type": "user_key"}}
	not authz.is_verified_user_key_client with input as {"user": {"azp": "ukey-foo", "principal_type": "user"}}
	not authz.is_verified_user_key_client with input as {"user": {"azp": "ukey-foo"}}
	not authz.is_verified_user_key_client with input as {"user": {"azp": "cyclops-cs-spa", "principal_type": "user_key"}}
}

# ── Parameter shapes ────────────────────────────────────────────────────────

test_valid_dns_label_accepts_labels if {
	authz.valid_dns_label("a")
	authz.valid_dns_label("my-pool")
	authz.valid_dns_label("pool123")
}

test_valid_dns_label_rejects_non_labels if {
	not authz.valid_dns_label("")
	not authz.valid_dns_label("My-Pool")
	not authz.valid_dns_label("-pool")
	not authz.valid_dns_label("pool-")
	not authz.valid_dns_label("pool_a")
	not authz.valid_dns_label("a/b")
	not authz.valid_dns_label(concat("", ["a" | some _ in numbers.range(1, 64)]))
}
