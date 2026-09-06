# /api/k8s/{path...} — kubectl-proxy passthrough.
#
# This surface is an allowlist. A request names a group, a version, a resource,
# a scope and a method, and unless some rule below names that combination it is
# denied. A resource nobody thought about is not reachable.
#
# It was a denylist until #6704: everything reached the Kubernetes API except
# the paths is_infra_k8s_path and is_event_k8s_path excluded. That posture is
# only as good as the exclusion list is complete, and completeness is not a
# property anyone can check — the failure mode is a resource nobody thought to
# exclude being proxied to the apiserver under the pod ServiceAccount. GitHub
# OIDC tokens were already allowlisted (github_k8s_request_allowed, below), so
# the file held two postures at once; this makes them one.
#
# What the allowlist admits was not guessed. It is the intersection of four
# sources of evidence, and the comment on each block says which:
#
#   1. 30 days of production traffic (335,742 requests over 60 distinct path
#      shapes), read from the backend's own request log and split by response
#      status so that requests the policy already denied are not mistaken for
#      working traffic.
#   2. cyclops-cs/sdk/src/routes.rs — the Rust SDK behind every sdk-bindings
#      language, which constructs these paths directly.
#   3. github_k8s_request_allowed, a reviewed statement of legitimate access.
#   4. The route tests and characterization cases in this repo.
#
# Admin is deliberately not a way around any of this. Under the denylist,
# is_admin was an escape hatch over the infra paths; an allowlist with an
# escape hatch is a denylist wearing a hat. Admins get the same paths everyone
# else does.
#
# Two denies still sit in front of the allowlist rather than being expressed as
# its absence. Both are redundant today — nothing below would admit an event or
# an infra path — and both are kept on purpose, so that widening the allowlist
# later cannot reopen them by accident.
#
# This surface also runs pool_admission.rego over the request body; see
# K8sRoutePolicy in policy_routes.go. That is a separate leaf, not a rule here,
# because it needs the raw body and nothing else on this surface does.
package authz_k8s

import data.authz

default allow = false

allow {
	not is_event_k8s_path(input.params.path)
	not is_infra_k8s_path(input.params.path)
	surface_allow
}

surface_allow {
	input.route == "/api/k8s/{path...}"
	authz.is_interactive_client
	k8s_request_allowed
}

# User API keys reach exactly what an interactive client reaches. They did
# before too — the difference is that "exactly what" is now enumerated.
surface_allow {
	input.route == "/api/k8s/{path...}"
	authz.is_user_key_client
	k8s_request_allowed
}

surface_allow {
	input.route == "/api/k8s/{path...}"
	input.user.principal_type == "github_oidc"
	github_k8s_request_allowed
}

# is_event_k8s_path matches every kubectl-proxy path that addresses the Event
# resource. `path` is `input.params.path`: the captured {path...} segment, no
# leading slash, no query string.
#
# There are eight such paths, not one, and all eight were checked against a live
# apiserver rather than inferred — two API groups serve the same objects, each
# under a cluster-scoped and a namespaced form, and each of those also has a
# legacy /watch/ alias that streams the same data:
#
#   api/v1/events[/name]                             apis/events.k8s.io/{v}/events[/name]
#   api/v1/namespaces/{ns}/events[/name]             apis/events.k8s.io/{v}/namespaces/{ns}/events[/name]
#   api/v1/watch/events                              apis/events.k8s.io/{v}/watch/events
#   api/v1/watch/namespaces/{ns}/events              apis/events.k8s.io/{v}/watch/namespaces/{ns}/events
#
# So this is not a substring test for "api/v1/events": that string occurs in one
# of the eight. Nor is it a test for the core group alone — events.k8s.io serves
# the identical objects, and a deny that named only core/v1 would read as
# "events are not proxied" while proxying them.
#
# The two axes are factored apart: event_api_prefix decides how many leading
# segments name the group and version, and the four rules below are the four
# addressing forms, written against that offset. Adding an API group is one more
# prefix rule; the forms do not have to be restated for it.
#
# Nothing here reads the query string, so ?watch=true is denied along with the
# list it decorates — as is the /watch/ path form, which is the same read under
# a different spelling.

# event_api_prefix reports how many leading segments a path spends naming a
# group and version that serves Events, and is undefined for a path that names
# neither. The version is matched positionally rather than against a literal, so
# a cluster serving v1beta1 — or a version that does not exist yet — is covered.
#
# parts[1] is compared whole, exactly as the Capsule rule above compares its
# group: "events.k8s.io.evil" is a different segment, not a prefix match.
event_api_prefix(parts) = 2 {
	parts[0] == "api"
	parts[1] == "v1"
}

event_api_prefix(parts) = 3 {
	parts[0] == "apis"
	parts[1] == "events.k8s.io"
	parts[2] != ""
}

# <prefix>/events[/name] — cluster-scoped collection or item.
is_event_k8s_path(path) {
	parts := split(path, "/")
	base := event_api_prefix(parts)
	count(parts) > base
	parts[base] == "events"
}

# <prefix>/namespaces/{ns}/events[/name] — namespaced collection or item. The
# namespace segment is matched positionally, so any value in it — including
# empty — still addresses an Event collection.
is_event_k8s_path(path) {
	parts := split(path, "/")
	base := event_api_prefix(parts)
	count(parts) > base + 2
	parts[base] == "namespaces"
	parts[base + 2] == "events"
}

# <prefix>/watch/events — the legacy cluster-scoped watch alias. Deprecated
# since Kubernetes 1.11 and still served: it returns a stream of whole Event
# objects, verified against the cluster this backend proxies to.
is_event_k8s_path(path) {
	parts := split(path, "/")
	base := event_api_prefix(parts)
	count(parts) > base + 1
	parts[base] == "watch"
	parts[base + 1] == "events"
}

# <prefix>/watch/namespaces/{ns}/events — the legacy namespaced watch alias.
is_event_k8s_path(path) {
	parts := split(path, "/")
	base := event_api_prefix(parts)
	count(parts) > base + 3
	parts[base] == "watch"
	parts[base + 1] == "namespaces"
	parts[base + 3] == "events"
}

# ── The allowlist ───────────────────────────────────────────────────────────
#
# k8s_request_allowed is the whole of what an interactive client or a user API
# key may ask the Kubernetes API for. Each block below names its evidence; a
# block with no evidence does not belong here.
#
# The shape helpers are shared with github_k8s_request_allowed further down,
# which has always worked this way — group, version, resource, scope, method.

# ── Fleet CRDs, native group ────────────────────────────────────────────────
#
# apis/osgym.cua.ai/v1alpha1/namespaces/{ns}/{claims,warmpools,templates,sandboxes}
#
# The resources this system exists to manage, and 99% of all observed traffic.
# Claims, warm pools, and templates use the existing CRUD matrix. Sandboxes are
# exposed separately as read-only instance state for the pool detail page.
native_fleet_resources := {
	"osgymsandboxclaims",
	"osgymsandboxwarmpools",
	"osgymsandboxtemplates",
}

k8s_request_allowed {
	parts := split(input.params.path, "/")
	apis_namespaced_group(parts, "osgym.cua.ai", "v1alpha1")
	native_fleet_resources[parts[5]]
	fleet_crud_shape(parts)
}

# Sandboxes are the instances behind a warm pool. The pool detail page lists
# them to show both available and claim-owned capacity, but never mutates them.
k8s_request_allowed {
	parts := split(input.params.path, "/")
	apis_namespaced_group(parts, "osgym.cua.ai", "v1alpha1")
	parts[5] == "osgymsandboxes"
	apis_read_shape(parts)
	input.method == "GET"
}

# The one write clients get on a Sandbox: PATCH on an item, so a claim holder
# can expose an extra service port on a RUNNING sandbox by appending to
# spec.vmTemplate.services (the pool-operator creates the matching per-sandbox
# Service; the VMI forwards all guest ports, so no restart is needed). This is
# a reviewed product decision (Fleet dynamic service exposure), not observed
# traffic. The body is NOT free-form: sandbox_services_admission.rego runs as
# a conjunct on this surface (see K8sRoutePolicy) and rejects any PATCH that
# touches more than spec.vmTemplate.services, so image, sizing and runtime
# stay operator-owned. Capsule scopes the caller to their own namespaces, the
# same as every other namespaced path here.
k8s_request_allowed {
	parts := split(input.params.path, "/")
	apis_namespaced_group(parts, "osgym.cua.ai", "v1alpha1")
	parts[5] == "osgymsandboxes"
	apis_item(parts)
	input.method == "PATCH"
}

# ── Fleet CRDs, legacy group ────────────────────────────────────────────────
#
# apis/cua.ai/v1/namespaces/{ns}/osgymworkspacepools
#
# The pre-osgym pool API, still carrying 22,926 GETs and a full write matrix in
# the last 30 days — it is not dead. PUT is included because production shows
# 40 successful PUTs: no rule ever named that verb, and it worked only because
# the denylist never asked. Naming it here is the first time anyone has decided
# it rather than defaulted into it.
k8s_request_allowed {
	parts := split(input.params.path, "/")
	apis_namespaced_group(parts, "cua.ai", "v1")
	parts[5] == "osgymworkspacepools"
	legacy_pool_shape(parts)
}

# ── Core workload reads ─────────────────────────────────────────────────────
#
# api/v1/namespaces/{ns}/{pods,services}
#
# Pods and their logs are what a customer looks at to see their own sandbox;
# policy_test.go has asserted them customer-facing since CUA-515. Reads only:
# nothing in traffic or in this repo writes a core resource through this proxy.
k8s_request_allowed {
	parts := split(input.params.path, "/")
	core_namespaced_resource(parts, "pods")
	core_read_shape(parts)
	input.method == "GET"
}

# Pod logs are a subresource, so they need the one extra segment.
k8s_request_allowed {
	parts := split(input.params.path, "/")
	core_namespaced_resource(parts, "pods")
	core_subresource(parts, "log")
	input.method == "GET"
}

k8s_request_allowed {
	parts := split(input.params.path, "/")
	core_namespaced_resource(parts, "services")
	core_read_shape(parts)
	input.method == "GET"
}

# ── Direct Service writes: denied with guidance ─────────────────────────────
#
# Nothing below admits a write to core Services, so a client that POSTs one —
# the obvious way to try to expose a new port on a sandbox — is denied either
# way. What this query adds is the reason: K8sRoutePolicy evaluates it wrapped
# in Because(...) BEFORE the allow leaf, so exactly these requests 403 with a
# message naming the supported alternative (spec.vmTemplate.services) instead
# of the generic "k8s request is not allowed". It changes no verdict: every
# request it denies, the allowlist denies too.
default not_direct_service_write = false

not_direct_service_write {
	not is_direct_service_write
}

is_direct_service_write {
	parts := split(input.params.path, "/")
	core_namespaced_resource(parts, "services")
	is_write_method
}

is_write_method {
	input.method == "POST"
}

is_write_method {
	input.method == "PUT"
}

is_write_method {
	input.method == "PATCH"
}

is_write_method {
	input.method == "DELETE"
}

# ── Pod metrics ─────────────────────────────────────────────────────────────
#
# apis/metrics.k8s.io/v1beta1/namespaces/{ns}/pods — the CPU/memory numbers
# shown next to a sandbox. Asserted customer-facing by policy_test.go.
k8s_request_allowed {
	parts := split(input.params.path, "/")
	apis_namespaced_group(parts, "metrics.k8s.io", "v1beta1")
	parts[5] == "pods"
	apis_read_shape(parts)
	input.method == "GET"
}

# ── KubeVirt reads ──────────────────────────────────────────────────────────
#
# apis/kubevirt.io/v1/namespaces/{ns}/{virtualmachines,virtualmachineinstances}
#
# The VMs behind the sandboxes; Capsule scopes the caller to their own
# namespaces. Reads only. Production also shows one DELETE of a VM and one
# /restart, both single occurrences that look like a person intervening by
# hand rather than a client's path — those stay denied until someone says
# otherwise, which is a smaller mistake than admitting a destructive verb on
# the strength of one call.
kubevirt_resources := {"virtualmachines", "virtualmachineinstances"}

k8s_request_allowed {
	parts := split(input.params.path, "/")
	apis_namespaced_group(parts, "kubevirt.io", "v1")
	kubevirt_resources[parts[5]]
	apis_read_shape(parts)
	input.method == "GET"
}

# ── Discovery ───────────────────────────────────────────────────────────────
#
# openapi/v2, openapi/v3/... and the bare group/version documents. This is
# kubectl's opening handshake and the SDKs' schema check: read-only, no object
# data, and denying it makes a client fail in a way that looks like an outage
# rather than a policy decision.
k8s_request_allowed {
	startswith(input.params.path, "openapi/")
	input.method == "GET"
}

k8s_request_allowed {
	parts := split(input.params.path, "/")
	count(parts) <= 3
	parts[0] == "apis"
	input.method == "GET"
}

# ── Permission self-check ───────────────────────────────────────────────────
#
# POST apis/authorization.k8s.io/v1/selfsubjectaccessreviews — asks the
# apiserver what the impersonated caller may do. It creates nothing and
# discloses only that answer, which the caller could establish anyway by
# trying. 12 successes in the last 30 days.
k8s_request_allowed {
	input.params.path == "apis/authorization.k8s.io/v1/selfsubjectaccessreviews"
	input.method == "POST"
}

# ── Shape helpers ───────────────────────────────────────────────────────────
#
# `input.params.path` is the captured {path...} segment: no leading slash, no
# query string. Segment indices are fixed because both path grammars are:
#
#   apis/{group}/{version}/namespaces/{ns}/{resource}[/{name}[/{sub}]]
#      0     1        2         3        4      5        6      7
#   api/v1/namespaces/{ns}/{resource}[/{name}[/{sub}]]
#     0   1     2       3      4         5       6

apis_namespaced_group(parts, group, version) {
	count(parts) >= 6
	parts[0] == "apis"
	parts[1] == group
	parts[2] == version
	parts[3] == "namespaces"
	parts[4] != ""
}

core_namespaced_resource(parts, resource) {
	count(parts) >= 5
	parts[0] == "api"
	parts[1] == "v1"
	parts[2] == "namespaces"
	parts[3] != ""
	parts[4] == resource
}

apis_collection(parts) {
	count(parts) == 6
}

apis_item(parts) {
	count(parts) == 7
	parts[6] != ""
}

apis_subresource(parts, name) {
	count(parts) == 8
	parts[6] != ""
	parts[7] == name
}

apis_read_shape(parts) {
	apis_collection(parts)
}

apis_read_shape(parts) {
	apis_item(parts)
}

core_read_shape(parts) {
	count(parts) == 5
}

core_read_shape(parts) {
	count(parts) == 6
	parts[5] != ""
}

core_subresource(parts, name) {
	count(parts) == 7
	parts[5] != ""
	parts[6] == name
}

# ── Verb matrices ───────────────────────────────────────────────────────────

fleet_crud_shape(parts) {
	apis_collection(parts)
	input.method == "GET"
}

fleet_crud_shape(parts) {
	apis_collection(parts)
	input.method == "POST"
}

fleet_crud_shape(parts) {
	apis_item(parts)
	input.method == "GET"
}

fleet_crud_shape(parts) {
	apis_item(parts)
	input.method == "PATCH"
}

fleet_crud_shape(parts) {
	apis_item(parts)
	input.method == "DELETE"
}

# The status subresource. 9,882 PATCHes in 30 days, every one of them refused
# — by Capsule's RBAC downstream, not by this policy, since the denylist
# admitted the shape. Enumerating it here changes nothing about that refusal;
# it is here so that the allowlist describes what callers actually ask for,
# and so the 403 stays a question about RBAC rather than becoming a question
# about this file.
fleet_crud_shape(parts) {
	apis_subresource(parts, "status")
	input.method == "PATCH"
}

legacy_pool_shape(parts) {
	fleet_crud_shape(parts)
}

legacy_pool_shape(parts) {
	apis_item(parts)
	input.method == "PUT"
}

# is_infra_k8s_path matches the kubectl-proxy paths that expose cluster
# internals. `input.params.path` is the captured {path...} segment with no
# leading slash (e.g. "api/v1/nodes"); query strings are not included.
#
# Every path it names is already outside the allowlist above, so this is
# redundant — deliberately. It is the conjunct that keeps a future widening of
# the allowlist from reopening the cluster's internals by accident, and it costs
# one predicate.
#
#   api/v1/nodes[/...]                       — NodesList (cluster capacity)
#   api/v1/pods                              — cluster-wide pod listing
#                                              (namespaced pods stay open)
#   apis/batch/v1/...                        — batch Jobs
#   api/v1/namespaces/cyclops-cs/configmaps  — the app's own config
#   apis/cua.ai/v1/osgymworkspacepools       — cluster-wide pool listing
#                                              (namespaced pools stay open)
#   apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims
#                                            — cluster-wide claim listing
#                                              (namespaced claims stay open)
#   apis/osgym.cua.ai/v1alpha1/{osgymsandboxwarmpools,osgymsandboxtemplates,osgymsandboxes}
#                                            — cluster-wide listings of the
#                                              native fleet CRDs (namespaced
#                                              access stays open)
#   apis/capsule.clastix.io/<version>/tenants
#                                            — cluster-wide Tenant ownership
#                                              and provisioning API, across
#                                              every served API version
#
# These cluster-wide CRD paths are defence-in-depth: the SPA only ever reads
# pools/claims per-namespace, and Tenant lifecycle is an internal backend
# operation. Blocking them here prevents the SPA proxy from reaching the
# backend ServiceAccount's cluster-wide capabilities.
# The legacy /watch/ alias serves the same collection as the path it aliases —
# api/v1/watch/pods streams exactly what api/v1/pods lists — so a deny that
# matched only the literal spelling did not cover it. Rather than restate every
# rule below in its watch form, the alias is rewritten to the path it aliases
# and the literal rules answer it. That makes agreement structural: a path and
# its alias cannot disagree, and an infra path added later is covered in both
# spellings without anyone remembering to.
#
# It also keeps the distinction that matters: api/v1/watch/namespaces/{ns}/pods
# rewrites to api/v1/namespaces/{ns}/pods, which is not an infra path and stays
# open. Namespaced pods are customer-facing in either spelling.
is_infra_k8s_path(path) {
	is_infra_literal(watch_alias_target(path))
}

is_infra_k8s_path(path) {
	is_infra_literal(path)
}

# watch_alias_target strips the /watch/ infix, in the two positions the K8s API
# puts it: after api/v1 for core resources, and after apis/{group}/{version} for
# everything else. Undefined for a path that is not a watch alias, which leaves
# the rule above undefined too.
watch_alias_target(path) = target {
	parts := split(path, "/")
	count(parts) > 3
	parts[0] == "api"
	parts[1] == "v1"
	parts[2] == "watch"
	target := concat("/", array.concat(["api", "v1"], array.slice(parts, 3, count(parts))))
}

watch_alias_target(path) = target {
	parts := split(path, "/")
	count(parts) > 4
	parts[0] == "apis"
	parts[3] == "watch"
	target := concat("/", array.concat(array.slice(parts, 0, 3), array.slice(parts, 4, count(parts))))
}

is_infra_literal(path) {
	path == "api/v1/nodes"
}

is_infra_literal(path) {
	startswith(path, "api/v1/nodes/")
}

is_infra_literal(path) {
	path == "api/v1/pods"
}

is_infra_literal(path) {
	startswith(path, "apis/batch/v1")
}

is_infra_literal(path) {
	startswith(path, "api/v1/namespaces/cyclops-cs/configmaps")
}

is_infra_literal(path) {
	path == "apis/cua.ai/v1/osgymworkspacepools"
}

is_infra_literal(path) {
	path == "apis/osgym.cua.ai/v1alpha1/osgymsandboxclaims"
}

is_infra_literal(path) {
	path == "apis/osgym.cua.ai/v1alpha1/osgymsandboxwarmpools"
}

is_infra_literal(path) {
	path == "apis/osgym.cua.ai/v1alpha1/osgymsandboxtemplates"
}

is_infra_literal(path) {
	path == "apis/osgym.cua.ai/v1alpha1/osgymsandboxes"
}

is_infra_literal(path) {
	parts := split(path, "/")
	count(parts) >= 4
	parts[0] == "apis"
	parts[1] == "capsule.clastix.io"
	parts[2] != ""
	parts[3] == "tenants"
}

github_k8s_request_allowed {
	parts := split(input.params.path, "/")
	github_legacy_pool_request(parts)
	github_namespace_allowed(parts[4])
}

github_k8s_request_allowed {
	parts := split(input.params.path, "/")
	github_native_fleet_request(parts)
	github_namespace_allowed(parts[4])
}

github_namespace_allowed(namespace) {
	input.user.allowed_namespaces[_] == namespace
}

github_legacy_pool_request(parts) {
	github_namespaced_resource(parts, "cua.ai", "v1", "osgymworkspacepools")
	github_collection_or_item(parts)
	input.method == "GET"
}

github_legacy_pool_request(parts) {
	github_namespaced_resource(parts, "cua.ai", "v1", "osgymworkspacepools")
	github_collection_or_item(parts)
	input.method == "POST"
}

github_legacy_pool_request(parts) {
	github_namespaced_resource(parts, "cua.ai", "v1", "osgymworkspacepools")
	github_collection_or_item(parts)
	input.method == "PATCH"
}

github_legacy_pool_request(parts) {
	github_namespaced_resource(parts, "cua.ai", "v1", "osgymworkspacepools")
	github_collection_or_item(parts)
	input.method == "DELETE"
}

github_native_fleet_request(parts) {
	github_namespaced_resource(parts, "osgym.cua.ai", "v1alpha1", "osgymsandboxclaims")
	github_native_crud_shape(parts)
}

github_native_fleet_request(parts) {
	github_namespaced_resource(parts, "osgym.cua.ai", "v1alpha1", "osgymsandboxwarmpools")
	github_native_crud_shape(parts)
}

github_native_fleet_request(parts) {
	github_namespaced_resource(parts, "osgym.cua.ai", "v1alpha1", "osgymsandboxtemplates")
	github_collection_or_item(parts)
	input.method == "GET"
}

github_native_crud_shape(parts) {
	github_collection_or_item(parts)
	input.method == "GET"
}

github_native_crud_shape(parts) {
	count(parts) == 6
	input.method == "POST"
}

github_native_crud_shape(parts) {
	github_nonempty_item(parts)
	input.method == "PATCH"
}

github_native_crud_shape(parts) {
	github_nonempty_item(parts)
	input.method == "DELETE"
}

github_namespaced_resource(parts, group, version, resource) {
	count(parts) >= 6
	parts[0] == "apis"
	parts[1] == group
	parts[2] == version
	parts[3] == "namespaces"
	parts[4] != ""
	parts[5] == resource
}

github_collection_or_item(parts) {
	count(parts) == 6
}

github_collection_or_item(parts) {
	github_nonempty_item(parts)
}

github_nonempty_item(parts) {
	count(parts) == 7
	parts[6] != ""
}
