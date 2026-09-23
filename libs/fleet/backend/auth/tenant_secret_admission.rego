# Admission over the request body for every Secret write /api/k8s admits.
#
# Tenants may create a few kinds of Secret through the gateway, each under its
# own name prefix. The allowlist (authz_k8s.rego) admits POST on the secrets
# collection and DELETE on an item whose name matches one of the
# tenant_secret_name_pattern entries. A POST names its object in the body, so
# the name and type checks live here: the collection path alone would let a
# tenant create any Secret in their namespace, including one the pool-operator
# reads (the OIDC credentials Secret) or a service-account-token Secret the
# token controller would fill with a live ServiceAccount credential.
#
# One module for all kinds, so the conjuncts never deny each other's Secrets:
#
#   * envelope: the checks every tenant Secret shares (POST on the collection,
#     only apiVersion/kind/metadata/type/data/stringData, metadata only
#     name/namespace/labels, no generateName/annotations/ownerReferences/
#     finalizers, namespace equal to the path's, string maps only);
#   * kind_admitted: one rule per kind. Each rule matches its own name prefix,
#     so kinds never overlap, and adds its type and payload checks.
#
# Adding a kind: add its name pattern to tenant_secret_name_pattern here AND in
# authz_k8s.rego (tenant_secret_admission_test.rego checks both sets agree),
# then add one kind_admitted rule below.
#
# Reads stay denied by the allowlist: tenant Secrets are write-only through the
# gateway. `applies` matches every write on the secrets resource,
# `allow { not applies }` passes the rest of the surface untouched, and
# anything that applies but is not an admitted kind is denied, so a later
# allowlist widening (PUT, PATCH) fails closed here.
package tenant_secret_admission

default allow = false

# ── Kind name patterns ──────────────────────────────────────────────────────

# Claim secrets: OSGymSandboxClaim spec.secretRef, delivered by the
# pool-operator into the bound sandbox (osgym/pool-operator/claim_secrets.py).
claim_secret_name_pattern := `^cua-claim-[a-z0-9]([-a-z0-9]*[a-z0-9])?$`

tenant_secret_name_pattern[pattern] {
	pattern := claim_secret_name_pattern
}

# ── Kinds ───────────────────────────────────────────────────────────────────

# Claim secret: a plain Opaque Secret (type omitted or "Opaque"), any string
# keys. The pool-operator re-checks the type before delivering it.
kind_admitted {
	name_matches(claim_secret_name_pattern)
	optional_equals(request_object, "type", "Opaque")
}

# ── Shared envelope ─────────────────────────────────────────────────────────

write_method {
	input.method == "POST"
}

write_method {
	input.method == "PUT"
}

write_method {
	input.method == "PATCH"
}

# api/v1/namespaces/{ns}/secrets[/{name}]
secret_resource_path {
	parts := split(input.params.path, "/")
	count(parts) >= 5
	count(parts) <= 6
	parts[0] == "api"
	parts[1] == "v1"
	parts[2] == "namespaces"
	parts[3] != ""
	parts[4] == "secrets"
}

applies {
	write_method
	secret_resource_path
}

allow {
	not applies
}

allow {
	applies
	envelope
	kind_admitted
}

request_object := json.unmarshal(input.body) {
	applies
}

allowed_top_level_keys := {"apiVersion", "kind", "metadata", "type", "data", "stringData"}

allowed_metadata_keys := {"name", "namespace", "labels"}

name_matches(pattern) {
	name := request_object.metadata.name
	is_string(name)
	count(name) <= 253
	regex.match(pattern, name)
}

optional_equals(obj, key, value) {
	not has_key(obj, key)
}

optional_equals(obj, key, value) {
	obj[key] == value
}

has_key(obj, key) {
	_ = obj[key]
}

string_map_or_absent(obj, key) {
	not has_key(obj, key)
}

string_map_or_absent(obj, key) {
	value := obj[key]
	is_object(value)
	not non_string_value(value)
}

non_string_value(value) {
	v := value[_]
	not is_string(v)
}

envelope {
	input.method == "POST"
	parts := split(input.params.path, "/")
	count(parts) == 5
	is_object(request_object)
	count(object.keys(request_object) - allowed_top_level_keys) == 0
	optional_equals(request_object, "apiVersion", "v1")
	optional_equals(request_object, "kind", "Secret")
	metadata := request_object.metadata
	is_object(metadata)
	count(object.keys(metadata) - allowed_metadata_keys) == 0
	optional_equals(metadata, "namespace", parts[3])
	string_map_or_absent(metadata, "labels")
	string_map_or_absent(request_object, "data")
	string_map_or_absent(request_object, "stringData")
}
