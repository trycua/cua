# Registry pull secret kind (cua-registry-*) of tenant_secret_admission.rego.
package tenant_secret_admission_registry_test

import data.tenant_secret_admission

collection_path := "api/v1/namespaces/ns-a/secrets"

post(body) := {
	"method": "POST",
	"params": {"path": collection_path},
	"body": body,
}

sdk_body := `{"apiVersion":"v1","kind":"Secret","type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-ghcr","namespace":"ns-a","labels":{"cua.ai/registry-secret":"true"}},"data":{".dockerconfigjson":"eyJhdXRocyI6e319"}}`

test_sdk_registry_secret_allowed {
	tenant_secret_admission.allow with input as post(sdk_body)
}

test_string_data_registry_secret_allowed {
	tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x"},"stringData":{".dockerconfigjson":"{\"auths\":{}}"}}`)
}

test_unrelated_request_allowed {
	tenant_secret_admission.allow with input as {
		"method": "POST",
		"params": {"path": "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates"},
		"body": "not json",
	}
}

test_delete_passes_through_to_allowlist {
	tenant_secret_admission.allow with input as {
		"method": "DELETE",
		"params": {"path": "api/v1/namespaces/ns-a/secrets/cua-registry-x"},
		"body": "",
	}
}

test_type_must_be_stated_dockerconfigjson {
	not tenant_secret_admission.allow with input as post(`{"metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x"},"data":{".dockerconfigjson":"e30="}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"Opaque","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x"},"data":{".dockerconfigjson":"e30="}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/service-account-token","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x"}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockercfg","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x"},"data":{".dockercfg":"e30="}}`)
}

test_other_names_denied {
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"name":"ecr-credentials"},"data":{".dockerconfigjson":"e30="}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-"},"data":{".dockerconfigjson":"e30="}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-X"},"data":{".dockerconfigjson":"e30="}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"generateName":"cua-registry-"},"data":{".dockerconfigjson":"e30="}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-claim-x"},"data":{".dockerconfigjson":"e30="}}`)
}

test_payload_must_be_exactly_the_docker_config {
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x"}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x"},"data":{".dockerconfigjson":"e30=","extra":"eA=="}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x"},"data":{".dockerconfigjson":"e30="},"stringData":{".dockerconfigjson":"{}"}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x"},"data":{".dockerconfigjson":1}}`)
}

test_metadata_and_extra_keys_denied {
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x","annotations":{"a":"b"}},"data":{".dockerconfigjson":"e30="}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x","ownerReferences":[]},"data":{".dockerconfigjson":"e30="}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x","namespace":"ns-b"},"data":{".dockerconfigjson":"e30="}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x"},"data":{".dockerconfigjson":"e30="},"immutable":true}`)
	not tenant_secret_admission.allow with input as post(`{"kind":"ConfigMap","type":"kubernetes.io/dockerconfigjson","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x"},"data":{".dockerconfigjson":"e30="}}`)
}

test_malformed_body_denied {
	not tenant_secret_admission.allow with input as post("not json")
}

test_post_to_item_put_and_patch_denied {
	not tenant_secret_admission.allow with input as {"method": "POST", "params": {"path": "api/v1/namespaces/ns-a/secrets/cua-registry-x"}, "body": sdk_body}
	not tenant_secret_admission.allow with input as {"method": "PUT", "params": {"path": "api/v1/namespaces/ns-a/secrets/cua-registry-x"}, "body": sdk_body}
	not tenant_secret_admission.allow with input as {"method": "PATCH", "params": {"path": "api/v1/namespaces/ns-a/secrets/cua-registry-x"}, "body": `{"data":{".dockerconfigjson":"e30="}}`}
}

test_managed_label_required {
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"name":"cua-registry-x"},"data":{".dockerconfigjson":"e30="}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"name":"cua-registry-x","labels":{"cua.ai/registry-secret":"false"}},"data":{".dockerconfigjson":"e30="}}`)
	not tenant_secret_admission.allow with input as post(`{"type":"kubernetes.io/dockerconfigjson","metadata":{"name":"cua-registry-x","labels":{"other":"true"}},"data":{".dockerconfigjson":"e30="}}`)
}

# The two kinds never admit each other's shape.
test_kinds_do_not_cross {
	not tenant_secret_admission.allow with input as post(`{"type":"Opaque","metadata":{"labels":{"cua.ai/registry-secret":"true"},"name":"cua-registry-x"},"stringData":{"env-token":"t"}}`)
	tenant_secret_admission.allow with input as post(`{"type":"Opaque","metadata":{"name":"cua-claim-x"},"stringData":{"env-token":"t"}}`)
}
