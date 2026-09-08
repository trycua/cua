# The one condition every authenticated route shares.
#
# These cases are the reason the base can be factored out at all: hoisting a
# check into a conjunct is only sound if the conjunct answers the same thing for
# every surface, so it is tested here once rather than re-tested per surface.
# What each surface still tests is that the composition denies an empty subject
# on its own routes, which is the property callers depend on.
package authz_base_test

import rego.v1

import data.authz_base

test_non_empty_subject_allowed if {
	authz_base.allow with input as {"user": {"sub": "user-123", "azp": "cyclops-cs-spa"}}
}

test_empty_subject_denied if {
	not authz_base.allow with input as {"user": {"sub": "", "azp": "cyclops-cs-spa"}}
}

test_missing_subject_denied if {
	not authz_base.allow with input as {"user": {"azp": "cyclops-cs-spa"}}
}

test_missing_user_denied if {
	not authz_base.allow with input as {"route": "/api/keys"}
}

# The base says nothing about which client the token came from. A surface that
# wants that says so itself — otherwise the check would be in two places and
# only one of them would get updated.
test_unknown_client_still_passes_the_base if {
	authz_base.allow with input as {"user": {"sub": "user-123", "azp": "untrusted-client"}}
}
