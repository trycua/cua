package auth

import (
	"net/http"
	"testing"
)

// These pin the per-claim secret surface end to end through the production
// tree: the SDK may create a plain Opaque cua-claim-* Secret and delete it by
// name, a create that is anything else 403s with its own message, and no other
// Secret operation reaches the apiserver.

const claimSecretCollection = "api/v1/namespaces/ns-a/secrets"

func TestK8sClaimSecretCreateAndDeleteAdmitted(t *testing.T) {
	setCardAdmissionFlags(t, false)
	const body = `{"apiVersion":"v1","kind":"Secret","type":"Opaque","metadata":{"name":"cua-claim-claim-a","labels":{"osgym.cua.ai/claim":"claim-a"}},"stringData":{"env-token":"t"}}`

	for _, testCase := range []struct {
		name   string
		method string
		path   string
		body   string
	}{
		{name: "create", method: http.MethodPost, path: claimSecretCollection, body: body},
		{name: "delete", method: http.MethodDelete, path: claimSecretCollection + "/cua-claim-claim-a"},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(testCase.method, testCase.path, testCase.body))
			if response.Code != http.StatusNoContent || !reached {
				t.Fatalf("status, reached = %d, %v; want %d, true", response.Code, reached, http.StatusNoContent)
			}
		})
	}
}

func TestK8sNonClaimSecretCreateReturnsRestrictedMessage(t *testing.T) {
	setCardAdmissionFlags(t, false)
	for _, testCase := range []struct {
		name string
		body string
	}{
		{name: "other name", body: `{"metadata":{"name":"workload-oidc"},"stringData":{"client_id":"x"}}`},
		{name: "generate name", body: `{"metadata":{"generateName":"cua-claim-"}}`},
		{name: "service account token", body: `{"type":"kubernetes.io/service-account-token","metadata":{"name":"cua-claim-x","annotations":{"kubernetes.io/service-account.name":"default"}}}`},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(http.MethodPost, claimSecretCollection, testCase.body))
			if response.Code != http.StatusForbidden || reached {
				t.Fatalf("status, reached = %d, %v; want %d, false", response.Code, reached, http.StatusForbidden)
			}
			if got := policyErrorMessage(t, response); got != TenantSecretRestrictedMessage {
				t.Fatalf("error = %q, want %q", got, TenantSecretRestrictedMessage)
			}
		})
	}
}

func TestK8sOtherSecretOperationsDenied(t *testing.T) {
	setCardAdmissionFlags(t, false)
	for _, testCase := range []struct {
		method string
		path   string
	}{
		{method: http.MethodGet, path: claimSecretCollection},
		{method: http.MethodGet, path: claimSecretCollection + "/cua-claim-claim-a"},
		{method: http.MethodPatch, path: claimSecretCollection + "/cua-claim-claim-a"},
		{method: http.MethodPut, path: claimSecretCollection + "/cua-claim-claim-a"},
		{method: http.MethodDelete, path: claimSecretCollection + "/ecr-credentials"},
		{method: http.MethodDelete, path: claimSecretCollection + "/osgym-claim-secrets-sandbox-1"},
	} {
		t.Run(testCase.method+" "+testCase.path, func(t *testing.T) {
			response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(testCase.method, testCase.path, `{"stringData":{"a":"b"}}`))
			if response.Code != http.StatusForbidden || reached {
				t.Fatalf("status, reached = %d, %v; want %d, false", response.Code, reached, http.StatusForbidden)
			}
		})
	}
}
