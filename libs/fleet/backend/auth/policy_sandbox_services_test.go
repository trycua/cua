package auth

import (
	"context"
	"net/http"
	"testing"
)

// These tests pin the two response contracts the Fleet dynamic-service-exposure
// work added to the k8s surface, end to end through the production tree and
// middleware options:
//
//   - A direct write to core Services 403s with a message naming the supported
//     alternative, instead of the generic surface denial. The verdict itself is
//     unchanged (the allowlist never admitted these); only the reason moved.
//   - PATCH on an osgymsandboxes item is admitted exactly when its body touches
//     nothing but spec.vmTemplate.services, and a body that strays 403s with
//     its own message rather than the generic one.
//
// The generic-denial case for unrelated paths stays pinned by
// TestK8sUnrelatedDenialKeepsSurfaceMessage in policy_card_admission_test.go.

func sandboxServicesRequest(method, path, body string) *http.Request {
	request := routeRequest(method, "/api/k8s/"+path, "/api/k8s/{path...}", map[string]string{"path": path}, body)
	ctx := context.WithValue(request.Context(), UserKey, &User{ID: "user-1", AZP: "cyclops-cs-spa", PrincipalType: PrincipalTypeUser})
	return request.WithContext(ctx)
}

func TestK8sDirectServiceWriteReturnsGuidance(t *testing.T) {
	setCardAdmissionFlags(t, false)
	const body = `{"apiVersion":"v1","kind":"Service","metadata":{"name":"sandbox-1-source-mcp"},"spec":{"type":"ClusterIP","selector":{"app":"sandbox-1"},"ports":[{"port":80,"targetPort":3100}]}}`

	for _, testCase := range []struct {
		name   string
		method string
		path   string
	}{
		{name: "create", method: http.MethodPost, path: "api/v1/namespaces/ns-a/services"},
		{name: "replace", method: http.MethodPut, path: "api/v1/namespaces/ns-a/services/sandbox-1-source-mcp"},
		{name: "patch", method: http.MethodPatch, path: "api/v1/namespaces/ns-a/services/sandbox-1-source-mcp"},
		{name: "delete", method: http.MethodDelete, path: "api/v1/namespaces/ns-a/services/sandbox-1-source-mcp"},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(testCase.method, testCase.path, body))
			if response.Code != http.StatusForbidden || reached {
				t.Fatalf("status, reached = %d, %v; want %d, false", response.Code, reached, http.StatusForbidden)
			}
			if got := policyErrorMessage(t, response); got != ServiceWriteNotSupportedMessage {
				t.Fatalf("error = %q, want %q", got, ServiceWriteNotSupportedMessage)
			}
		})
	}
}

// Reading Services stays allowed, so the guidance conjunct provably denies
// writes alone.
func TestK8sServiceReadStillAllowed(t *testing.T) {
	setCardAdmissionFlags(t, false)
	response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(http.MethodGet, "api/v1/namespaces/ns-a/services", ""))
	if response.Code != http.StatusNoContent || !reached {
		t.Fatalf("status, reached = %d, %v; want %d, true", response.Code, reached, http.StatusNoContent)
	}
}

func TestK8sSandboxServicesPatchAdmitted(t *testing.T) {
	setCardAdmissionFlags(t, false)
	const path = "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes/sandbox-1"
	const body = `{"spec":{"vmTemplate":{"services":[{"name":"source-mcp","targetPort":3100}]}}}`

	response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(http.MethodPatch, path, body))
	if response.Code != http.StatusNoContent || !reached {
		t.Fatalf("status, reached = %d, %v; want %d, true", response.Code, reached, http.StatusNoContent)
	}
}

func TestK8sSandboxPatchBeyondServicesReturnsRestrictedMessage(t *testing.T) {
	setCardAdmissionFlags(t, false)
	const path = "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxes/sandbox-1"
	const body = `{"spec":{"vmTemplate":{"containerDiskImage":"evil.example/workspace:latest"}}}`

	response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(http.MethodPatch, path, body))
	if response.Code != http.StatusForbidden || reached {
		t.Fatalf("status, reached = %d, %v; want %d, false", response.Code, reached, http.StatusForbidden)
	}
	if got := policyErrorMessage(t, response); got != SandboxPatchRestrictedMessage {
		t.Fatalf("error = %q, want %q", got, SandboxPatchRestrictedMessage)
	}
}
