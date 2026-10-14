# The check every authenticated route runs, whatever surface it belongs to.
#
# All 30 route-gated allow rules of the former monolith required a non-empty
# subject and nothing else in common, so hoisting that one condition out is what
# makes the rest decomposable: for a request on route R,
#
#     All(authz_base.allow, surface_R.allow)  ≡  the old authz.allow
#
# because every rule was route-gated, so no other surface's rules could have
# fired on R anyway, and each surviving rule is its old body minus this check.
#
# What "non-empty subject" means here is narrow, and worth being precise about:
# the request carries a validated token (TokenAuthMiddleware rejects it
# otherwise, with a 401, before this ever runs) whose identity resolved to
# something. An empty sub reaches this point from a token with no sub claim, or
# from a user-key whose owner could not be resolved — authenticated as a token,
# anonymous as a principal. Authorization proper is the surface's job.
package authz_base

default allow = false

allow {
	input.user.sub != ""
}
