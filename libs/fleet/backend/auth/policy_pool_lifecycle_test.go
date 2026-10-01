package auth

import (
	"net/http"
	"testing"
)

// These tests pin that the pool lifecycle fields (WarmPool spec.idleTtlSeconds
// and spec.ttlPolicy) and creator-set cua.ai/ labels and annotations on warm
// pools and claims pass the production /api/k8s tree end to end. The surface is
// an allowlist over group/resource/method, and only templates and sandboxes
// have body admission, so no policy change is needed for them. These tests
// fail if a future body leaf starts rejecting the lifecycle fields.

func TestK8sWarmPoolLifecycleFieldsAdmitted(t *testing.T) {
	setCardAdmissionFlags(t, false)
	const collection = "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxwarmpools"
	const item = collection + "/ns-a"

	for _, testCase := range []struct {
		name   string
		method string
		path   string
		body   string
	}{
		{
			name:   "create with idle TTL, Cascade policy, labels and annotations",
			method: http.MethodPost,
			path:   collection,
			body: `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxWarmPool",` +
				`"metadata":{"name":"ns-a","namespace":"ns-a",` +
				`"labels":{"cua.ai/managed-by":"cua-sdk","cua.ai/owner-hash":"abc123"},` +
				`"annotations":{"cua.ai/created-by":"sdk"}},` +
				`"spec":{"replicas":0,"sandboxTemplateRef":{"name":"ns-a-template"},` +
				`"autoscaling":{"minPoolSize":0,"maxPoolSize":4},` +
				`"idleTtlSeconds":900,"ttlPolicy":"Cascade","ttlSecondsAfterCreated":86400}}`,
		},
		{
			name:   "merge-patch idle TTL and policy",
			method: http.MethodPatch,
			path:   item,
			body:   `{"spec":{"idleTtlSeconds":300,"ttlPolicy":"Retain"}}`,
		},
		{
			name:   "merge-patch metadata only",
			method: http.MethodPatch,
			path:   item,
			body:   `{"metadata":{"labels":{"cua.ai/managed-by":"cua-sdk"},"annotations":{"cua.ai/last-used":"2026-09-22T00:00:00Z"}}}`,
		},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(testCase.method, testCase.path, testCase.body))
			if response.Code != http.StatusNoContent || !reached {
				t.Fatalf("status, reached = %d, %v; want %d, true; body = %s",
					response.Code, reached, http.StatusNoContent, response.Body.String())
			}
		})
	}
}

func TestK8sClaimCreatorLabelsAdmitted(t *testing.T) {
	setCardAdmissionFlags(t, false)
	const collection = "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims"

	for _, testCase := range []struct {
		name   string
		method string
		path   string
		body   string
	}{
		{
			name:   "create with labels",
			method: http.MethodPost,
			path:   collection,
			body: `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim",` +
				`"metadata":{"name":"claim-a","namespace":"ns-a","labels":{"cua.ai/fleet":"f1"}},` +
				`"spec":{"sandboxTemplateRef":{"name":"ns-a-template"},"ttlSecondsAfterCreated":600}}`,
		},
		{
			name:   "patch labels and annotations",
			method: http.MethodPatch,
			path:   collection + "/claim-a",
			body:   `{"metadata":{"labels":{"cua.ai/fleet":"f2"},"annotations":{"cua.ai/note":"x"}}}`,
		},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(testCase.method, testCase.path, testCase.body))
			if response.Code != http.StatusNoContent || !reached {
				t.Fatalf("status, reached = %d, %v; want %d, true; body = %s",
					response.Code, reached, http.StatusNoContent, response.Body.String())
			}
		})
	}
}
