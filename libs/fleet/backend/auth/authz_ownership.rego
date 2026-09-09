# Namespace ownership — the tenancy boundary for the routes that name a
# namespace in their path.
#
# This is the one authorization question the policy could not answer before,
# because answering it needs something no token carries: whether Kubernetes
# would let this subject act in that namespace. It lived in Go
# (handlers.requireNamespaceAccess) for exactly that reason. It lives here now
# because the missing half arrives as `input.facts.namespace_rbac`, loaded by a
# FactProvider that wraps the same impersonated RoleBinding probe the handler
# ran — see NamespaceOwnershipPolicy in policy_routes.go.
#
# The decision is three cases, and they are three rules rather than one because
# only the third costs a network round trip:
#
#   1. GitHub OIDC principals — the trust policy's allowed_namespaces claim IS
#      the grant. The token was minted against a policy that named them.
#   2. Per-key clients — bound to one namespace by their `namespace` claim.
#   3. Everyone else (SPA users, user keys, oauth2-proxy sessions, MCP
#      connector clients) — must hold RBAC in the namespace.
#
# Cases 1 and 2 are claim_allow; case 3 is probe_eligible AND rbac_allow. The
# split is what keeps the probe off the request path for everyone the claims
# already answer for: the plan is Any(claim_allow, All(probe_eligible,
# rbac_allow)), and a fold that stops at the first `true` never evaluates the
# leaf carrying the fact provider, so the fact is never loaded. probe_eligible
# is the same trick in the other direction — a principal the probe cannot speak
# for (a GitHub token, a per-key service account) denies the conjunction before
# the loader is reached.
#
# The order of the cases matters and matches the Go code it replaces: a GitHub
# principal is judged by its claim even if its azp also looked like a per-key
# client's, which is why cases 2 and 3 exclude it explicitly rather than relying
# on the families being disjoint.
package authz_ownership

import data.authz

default claim_allow = false

default probe_eligible = false

default rbac_allow = false

# ── Where the boundary applies ──────────────────────────────────────────────
#
# These are exactly the remaining call sites requireNamespaceAccess had. The
# /api/svc proxy dials {service}.{namespace}.svc.cluster.local directly, so
# Capsule cannot scope it; /api/namespaces/{name} GET reads one namespace
# through the K8s API and wants the same answer without trusting the read.
#
# DELETE on /api/namespaces/{name} is deliberately absent: it never ran this
# check. It goes to the K8s API impersonated, where Capsule is the boundary, and
# its own GitHub-claim check lives in the handler.
applies {
	input.route == "/api/svc/{namespace}/{service}"
}

applies {
	input.route == "/api/svc/{namespace}/{service}/{path...}"
}

applies {
	input.route == "/api/namespaces/{name}"
	input.method == "GET"
}

# Signed service URL management uses the backend service account, so it must
# prove namespace ownership here before the handler acts.
applies {
	input.route == "/api/signed-service-urls/{namespace}"
}

applies {
	input.route == "/api/signed-service-urls/{namespace}/{id}"
}

# target_namespace is the namespace this request is about: /api/svc names it
# {namespace}, /api/namespaces/{name} names it {name}. Keyed
# off which parameter the route bound rather than off the route itself, so the
# two lists cannot disagree — and mirrored in Go by auth.OwnedNamespace, which
# is what the fact provider probes and what TestOwnedNamespaceMatchesRego pins
# against these rules.
#
# An empty {namespace} still takes the first rule: "" is a defined value, so the
# `not` in the second is false. Every rule below requires a non-empty namespace,
# so the empty case denies rather than falling through to {name}.
target_namespace = input.params.namespace {
	input.params.namespace
}

target_namespace = input.params.name {
	not input.params.namespace
}

# ── The three cases ─────────────────────────────────────────────────────────

# GitHub OIDC: the claim is the grant, and no probe could improve on it — the
# impersonated subject is the owner who installed the trust policy, not the
# workflow calling through it.
claim_allow {
	applies
	is_github_principal
	input.user.allowed_namespaces[_] == target_namespace
}

# Per-key clients: the claim IS the grant. Their sub is a Keycloak service
# account that owns nothing, so a probe would deny a legitimate caller.
claim_allow {
	applies
	not is_github_principal
	authz.is_per_key_client
	target_namespace != ""
	input.user.namespace == target_namespace
}

# A route this boundary does not cover passes it unconditionally. The conjunct
# is attached per surface, and a surface can hold routes on both sides of the
# boundary — /api/namespaces is one — so "does not apply" has to be an allow
# here rather than an absence there.
claim_allow {
	not applies
}

# probe_eligible reports whether a K8s RBAC probe can speak for this principal
# at all. It is a leaf of its own so that All(probe_eligible, rbac_allow) can
# deny before the fact provider is asked: this is the cheap check the optimizer
# sinks the loader behind.
probe_eligible {
	applies
	not is_github_principal
	not authz.is_per_key_client
	not authz.is_legacy_per_key_client
	target_namespace != ""
}

# The probe's answer. probe_eligible is restated rather than assumed, because a
# rule that reads a fact must be safe to evaluate on its own — the plan's
# ordering is a performance property, not a correctness one.
rbac_allow {
	probe_eligible
	input.facts.namespace_rbac.allowed == true
}

is_github_principal {
	input.user.principal_type == "github_oidc"
}
