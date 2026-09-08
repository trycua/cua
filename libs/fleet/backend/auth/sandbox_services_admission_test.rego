package sandbox_services_admission_test

import data.sandbox_services_admission

item_path := "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes/sandbox-1"

collection_path := "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes"

services_patch := `{"spec":{"vmTemplate":{"services":[{"name":"source-mcp","targetPort":3100}]}}}`

patch_input(path, body) := {
	"method": "PATCH",
	"params": {"path": path},
	"body": body,
}

# Requests off this module's resource pass through untouched, whatever the
# body says — the module gates sandbox writes and nothing else.
test_unrelated_request_allowed {
	sandbox_services_admission.allow with input as {
		"method": "POST",
		"params": {"path": "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools"},
		"body": "not json",
	}
}

test_sandbox_read_allowed {
	sandbox_services_admission.allow with input as {
		"method": "GET",
		"params": {"path": item_path},
		"body": "",
	}
}

# The one permitted shape: a merge patch that names spec.vmTemplate.services
# and nothing else.
test_services_only_patch_allowed {
	sandbox_services_admission.allow with input as patch_input(item_path, services_patch)
}

test_services_patch_with_resource_version_allowed {
	sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"metadata":{"resourceVersion":"12345"},"spec":{"vmTemplate":{"services":[{"name":"source-mcp","targetPort":3100,"protocol":"TCP"}]}}}`,
	)
}

# Clearing the extra services is spelled with an empty array, which is a
# services-only patch like any other.
test_empty_services_patch_allowed {
	sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"spec":{"vmTemplate":{"services":[]}}}`,
	)
}

test_malformed_body_denied {
	not sandbox_services_admission.allow with input as patch_input(item_path, "not json")
}

# A JSON Patch body is an array, not an object, and can address arbitrary
# fields — only the merge-patch object shape is admitted.
test_json_patch_array_denied {
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`[{"op":"replace","path":"/spec/vmTemplate/containerDiskImage","value":"evil.example/x"}]`,
	)
}

# Anything beyond the services field is operator-owned.
test_image_patch_denied {
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"spec":{"vmTemplate":{"containerDiskImage":"evil.example/workspace:latest"}}}`,
	)
}

test_services_plus_image_patch_denied {
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"spec":{"vmTemplate":{"services":[{"name":"mcp","targetPort":3100}],"cpuCores":64}}}`,
	)
}

test_status_smuggled_alongside_spec_denied {
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"spec":{"vmTemplate":{"services":[]}},"status":{"phase":"Ready"}}`,
	)
}

# A merge-patch null is a delete; a null smuggled at the top level must fail
# the key-set comparison rather than slip past a truthiness filter.
test_null_valued_key_denied {
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"spec":{"vmTemplate":{"services":[]}},"status":null}`,
	)
}

test_metadata_beyond_resource_version_denied {
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"metadata":{"resourceVersion":"1","labels":{"osgym.cua.ai/warmpool":"stolen"}},"spec":{"vmTemplate":{"services":[]}}}`,
	)
}

test_services_must_be_an_array {
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"spec":{"vmTemplate":{"services":null}}}`,
	)
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"spec":{"vmTemplate":{"services":false}}}`,
	)
}

# Each entry becomes a real Kubernetes Service; the cap bounds the churn one
# sandbox can demand.
test_service_count_cap_enforced {
	over_cap := json.marshal({"spec": {"vmTemplate": {"services": [{"name": sprintf("svc-%d", [i]), "targetPort": 3000 + i} | i := numbers.range(1, 33)[_]]}}})
	not sandbox_services_admission.allow with input as patch_input(item_path, over_cap)

	at_cap := json.marshal({"spec": {"vmTemplate": {"services": [{"name": sprintf("svc-%d", [i]), "targetPort": 3000 + i} | i := numbers.range(1, 32)[_]]}}})
	sandbox_services_admission.allow with input as patch_input(item_path, at_cap)
}

# The collection takes no writes, and non-PATCH verbs on the item stay denied
# here even with a services-shaped body — fail-closed against a future
# allowlist widening.
test_collection_write_denied {
	not sandbox_services_admission.allow with input as {
		"method": "POST",
		"params": {"path": collection_path},
		"body": services_patch,
	}
}

test_put_denied_even_with_services_body {
	not sandbox_services_admission.allow with input as {
		"method": "PUT",
		"params": {"path": item_path},
		"body": services_patch,
	}
}

# The operator creates a Service literally named `<sandbox>-<name>`, so a
# name the apiserver would reject must be denied here, at the API boundary,
# instead of wedging the operator's field handler in a retry loop.
test_invalid_service_name_charset_denied {
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"spec":{"vmTemplate":{"services":[{"name":"Source_MCP","targetPort":3100}]}}}`,
	)
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"spec":{"vmTemplate":{"services":[{"name":"-mcp","targetPort":3100}]}}}`,
	)
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"spec":{"vmTemplate":{"services":[{"name":"","targetPort":3100}]}}}`,
	)
}

# `<sandbox>-<name>` must fit an RFC-1123 label: 63 characters. The sandbox
# name comes from the path ("sandbox-1", 9 chars), leaving 53 for the suffix.
test_combined_service_name_length_bounded {
	name_53 := concat("", [x | numbers.range(1, 53)[_]; x := "b"])
	at_limit := json.marshal({"spec": {"vmTemplate": {"services": [{"name": name_53, "targetPort": 3100}]}}})
	sandbox_services_admission.allow with input as patch_input(item_path, at_limit)

	name_54 := concat("", [x | numbers.range(1, 54)[_]; x := "b"])
	over_limit := json.marshal({"spec": {"vmTemplate": {"services": [{"name": name_54, "targetPort": 3100}]}}})
	not sandbox_services_admission.allow with input as patch_input(item_path, over_limit)
}

# The reconciler keys Services by name; duplicates would silently collapse to
# the last entry, so they are rejected instead.
test_duplicate_service_names_denied {
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"spec":{"vmTemplate":{"services":[{"name":"mcp","targetPort":3100},{"name":"mcp","targetPort":3200}]}}}`,
	)
}

test_entry_without_name_denied {
	not sandbox_services_admission.allow with input as patch_input(
		item_path,
		`{"spec":{"vmTemplate":{"services":[{"targetPort":3100}]}}}`,
	)
}
