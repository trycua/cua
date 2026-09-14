# Admission over the request body for the one write /api/k8s admits on
# osgymsandboxes: a PATCH whose body touches nothing but
# spec.vmTemplate.services, plus an optional metadata.resourceVersion CAS
# precondition. Everything else on the Sandbox spec — image, pull secret,
# sizing, runtime, probes, OIDC — stays operator-owned, so this module is what
# keeps "clients may expose a port on their sandbox" from quietly becoming
# "clients may rewrite their sandbox".
#
# It runs as a conjunct of K8sRoutePolicy (policy_routes.go), the same shape as
# pool_admission.rego: `applies` matches the write shapes on the resource this
# module owns, `allow { not applies }` passes everything else on the surface
# untouched, and an applying request must prove its body is the one permitted
# shape. That makes it fail-closed against future allowlist widening too — a
# POST or PUT on osgymsandboxes admitted later would land here and be denied
# until someone decides what its body may say.
#
# The service entries' types (name a string, targetPort 1-65535, protocol an
# enum) are validated by the CRD's structural schema at the apiserver, so this
# module mostly pins the SHAPE of the patch — with two exceptions the schema
# does not cover:
#
#   - a count cap, because each entry becomes a real Kubernetes Service and
#     the schema has no maxItems;
#   - name validity and uniqueness, because the operator creates a Service
#     literally named `<sandbox>-<name>` (an RFC-1123 DNS label of at most 63
#     characters) and keys its reconcile by `name`. A name the apiserver would
#     reject must 403 here, at the API boundary, rather than wedge the
#     operator's field handler in a retry loop against an impossible create;
#     duplicates would silently collapse to the last entry.
package sandbox_services_admission

default allow = false

# The most services one sandbox may declare. Each entry costs a Kubernetes
# Service object; a real sandbox exposes a handful.
max_services := 32

write_method {
	input.method == "POST"
}

write_method {
	input.method == "PUT"
}

write_method {
	input.method == "PATCH"
}

# apis/osgym.cua.ai/v1alpha1/namespaces/{ns}/osgymsandboxes — the collection.
sandbox_resource_path {
	parts := split(input.params.path, "/")
	count(parts) == 6
	parts[0] == "apis"
	parts[1] == "osgym.cua.ai"
	parts[2] == "v1alpha1"
	parts[3] == "namespaces"
	parts[4] != ""
	parts[5] == "osgymsandboxes"
}

# apis/osgym.cua.ai/v1alpha1/namespaces/{ns}/osgymsandboxes/{name} — an item.
sandbox_resource_path {
	parts := split(input.params.path, "/")
	count(parts) == 7
	parts[0] == "apis"
	parts[1] == "osgym.cua.ai"
	parts[2] == "v1alpha1"
	parts[3] == "namespaces"
	parts[4] != ""
	parts[5] == "osgymsandboxes"
	parts[6] != ""
}

applies {
	write_method
	sandbox_resource_path
}

allow {
	not applies
}

request_object := json.unmarshal(input.body) {
	applies
}

# The body's top level may name spec alone, or spec plus a metadata that
# carries only the resourceVersion precondition (the optimistic-concurrency
# handle for read-modify-write of the services array; JSON merge patch
# replaces the whole array, so concurrent blind writes would clobber).
#
# object.keys sees every key whatever its value, so a smuggled
# `"status": null` (a merge-patch delete) or `"metadata": {"labels": ...}`
# fails the comparison rather than slipping past a truthiness filter.
top_level_shape_allowed {
	object.keys(request_object) == {"spec"}
}

top_level_shape_allowed {
	object.keys(request_object) == {"metadata", "spec"}
	metadata := request_object.metadata
	is_object(metadata)
	object.keys(metadata) == {"resourceVersion"}
	is_string(metadata.resourceVersion)
}

services_only_patch {
	is_object(request_object)
	top_level_shape_allowed
	spec := request_object.spec
	is_object(spec)
	object.keys(spec) == {"vmTemplate"}
	vm_template := spec.vmTemplate
	is_object(vm_template)
	object.keys(vm_template) == {"services"}
	services := vm_template.services
	is_array(services)
	count(services) <= max_services
	service_names_valid(services)
}

# Every entry must carry a name the apiserver will accept as part of the
# Service name `<sandbox>-<name>` (RFC-1123 DNS label, ≤ 63 characters
# combined with the sandbox name from the path), and no two entries may share
# a name. Entries are checked by negation — one bad entry fails the whole
# patch — and an entry with no name (or a non-string one) counts as bad, the
# same verdict the CRD schema would reach one hop later.
service_names_valid(services) {
	names := {name | name := services[_].name}
	count(names) == count(services)
	not any_entry_name_invalid(services)
}

any_entry_name_invalid(services) {
	entry := services[_]
	not entry_name_valid(entry)
}

entry_name_valid(entry) {
	is_object(entry)
	name := entry.name
	is_string(name)
	regex.match(`^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`, name)
	parts := split(input.params.path, "/")
	sandbox_name := parts[6]
	(count(sandbox_name) + 1) + count(name) <= 63
}

# Only the PATCH verb ever reaches `applies` today (the allowlist admits no
# other sandbox write), but the check is explicit so an admitted POST or PUT
# cannot ride through on a services-shaped body.
allow {
	applies
	input.method == "PATCH"
	services_only_patch
}
