package tenant_secret_admission_test

import data.authz_k8s
import data.tenant_secret_admission

collection_path := "api/v1/namespaces/ns-a/secrets"

post(body) := {
	"method": "POST",
	"params": {"path": collection_path},
	"body": body,
}

sdk_body := `{"apiVersion":"v1","kind":"Secret","type":"Opaque","metadata":{"name":"cua-claim-claim-a","namespace":"ns-a","labels":{"osgym.cua.ai/claim":"claim-a"}},"stringData":{"env-token":"t"}}`

test_sdk_claim_secret_allowed {
	tenant_secret_admission.allow with input as post(sdk_body)
}

test_minimal_claim_secret_allowed {
	tenant_secret_admission.allow with input as post(`{"metadata":{"name":"cua-claim-x"},"data":{"env-token":"dA=="}}`)
}

test_unrelated_request_allowed {
	tenant_secret_admission.allow with input as {
		"method": "POST",
		"params": {"path": "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims"},
		"body": "not json",
	}
}

test_delete_passes_through_to_allowlist {
	tenant_secret_admission.allow with input as {
		"method": "DELETE",
		"params": {"path": "api/v1/namespaces/ns-a/secrets/cua-claim-x"},
		"body": "",
	}
}

test_other_name_denied {
	not tenant_secret_admission.allow with input as post(`{"metadata":{"name":"ecr-credentials"},"stringData":{"a":"b"}}`)
}

test_oidc_credentials_name_denied {
	not tenant_secret_admission.allow with input as post(`{"metadata":{"name":"workload-oidc"},"stringData":{"client_id":"x"}}`)
}

test_bare_prefix_denied {
	not tenant_secret_admission.allow with input as post(`{"metadata":{"name":"cua-claim-"}}`)
}

test_uppercase_name_denied {
	not tenant_secret_admission.allow with input as post(`{"metadata":{"name":"cua-claim-X"}}`)
}

test_generate_name_denied {
	not tenant_secret_admission.allow with input as post(`{"metadata":{"generateName":"cua-claim-"}}`)
}

test_generate_name_alongside_name_denied {
	not tenant_secret_admission.allow with input as post(`{"metadata":{"name":"cua-claim-x","generateName":"cua-claim-"}}`)
}

test_service_account_token_type_denied {
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/service-account-token","metadata":{"name":"cua-claim-x","annotations":{"kubernetes.io/service-account.name":"default"}}}`)
}

test_service_account_token_type_without_annotations_denied {
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/service-account-token","metadata":{"name":"cua-claim-x"}}`)
}

test_dockerconfig_type_denied {
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"name":"cua-claim-x"}}`)
}

test_annotations_denied {
	not tenant_secret_admission.allow with input as post(`{"metadata":{"name":"cua-claim-x","annotations":{"a":"b"}}}`)
}

test_owner_references_denied {
	not tenant_secret_admission.allow with input as post(`{"metadata":{"name":"cua-claim-x","ownerReferences":[]}}`)
}

test_other_namespace_in_body_denied {
	not tenant_secret_admission.allow with input as post(`{"metadata":{"name":"cua-claim-x","namespace":"ns-b"}}`)
}

test_wrong_kind_denied {
	not tenant_secret_admission.allow with input as post(`{"kind":"ConfigMap","metadata":{"name":"cua-claim-x"}}`)
}

test_extra_top_level_key_denied {
	not tenant_secret_admission.allow with input as post(`{"metadata":{"name":"cua-claim-x"},"immutable":true}`)
}

test_non_string_data_denied {
	not tenant_secret_admission.allow with input as post(`{"metadata":{"name":"cua-claim-x"},"stringData":{"a":1}}`)
}

test_malformed_body_denied {
	not tenant_secret_admission.allow with input as post("not json")
}

test_post_to_item_path_denied {
	not tenant_secret_admission.allow with input as {
		"method": "POST",
		"params": {"path": "api/v1/namespaces/ns-a/secrets/cua-claim-x"},
		"body": `{"metadata":{"name":"cua-claim-x"}}`,
	}
}

test_put_and_patch_denied {
	not tenant_secret_admission.allow with input as {
		"method": "PUT",
		"params": {"path": "api/v1/namespaces/ns-a/secrets/cua-claim-x"},
		"body": `{"metadata":{"name":"cua-claim-x"}}`,
	}
	not tenant_secret_admission.allow with input as {
		"method": "PATCH",
		"params": {"path": "api/v1/namespaces/ns-a/secrets/cua-claim-x"},
		"body": `{"stringData":{"env-token":"t"}}`,
	}
}

# authz_k8s.rego admits DELETE by the same name patterns this module admits
# creation under. A kind added to one set but not the other would be
# creatable but not deletable (or the reverse).
test_name_patterns_agree_with_allowlist {
	authz_k8s.tenant_secret_name_pattern == tenant_secret_admission.tenant_secret_name_pattern
	count(tenant_secret_admission.tenant_secret_name_pattern) > 0
}

# Kinds must not overlap: a kind's sample name matches exactly one pattern, so
# only that kind's type and payload rules can admit it. Add a sample per kind.
kind_sample_names := {"cua-claim-x"}

test_each_kind_name_matches_exactly_one_pattern {
	count({n | n := kind_sample_names[_]; count({p | p := tenant_secret_admission.tenant_secret_name_pattern[_]; regex.match(p, n)}) != 1}) == 0
}
