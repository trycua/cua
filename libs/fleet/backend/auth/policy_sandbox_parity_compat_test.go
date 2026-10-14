package auth

import (
	"net/http"
	"testing"
)

// Request shapes older SDKs (cyclops-sdk / Fleet SDK releases, terraform
// provider, @trycua/*) send today. The sandbox-parity admission conjuncts
// (sandbox_process_admission.rego, the cua-registry-* kind of
// tenant_secret_admission.rego, the pool_admission registry rules) must give
// every one of them the verdict it had before: admitted shapes stay admitted,
// denied shapes stay denied.
func TestK8sOldSDKRequestShapesKeepTheirVerdict(t *testing.T) {
	setCardAdmissionFlags(t, false)
	const templates = "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates"
	const pools = "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxwarmpools"
	for _, testCase := range []struct {
		name    string
		method  string
		path    string
		body    string
		allowed bool
	}{
		{
			name:    "kubevirt template with public ECR image",
			method:  http.MethodPost,
			path:    templates,
			body:    `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"containerDiskImage":"public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-38352d34","cpuCores":4,"memory":"8Gi","services":[{"name":"api","port":8000,"targetPort":8000}]}}}`,
			allowed: true,
		},
		{
			name:    "kubevirt template with a command (ignored on kubevirt, as before)",
			method:  http.MethodPost,
			path:    templates,
			body:    `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"containerDiskImage":"public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-38352d34","command":["/start.sh"]}}}`,
			allowed: true,
		},
		{
			name:    "gvisor template with command and the shared ECR pull secret",
			method:  http.MethodPost,
			path:    templates,
			body:    `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"runtime":"gvisor","containerDiskImage":"296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace:main","imagePullSecret":"ecr-credentials","command":["/entrypoint.sh"]}}}`,
			allowed: true,
		},
		{
			name:    "template merge patch that clears the pull secret",
			method:  http.MethodPatch,
			path:    templates + "/t",
			body:    `{"spec":{"vmTemplate":{"cpuCores":2,"imagePullSecret":null}}}`,
			allowed: true,
		},
		{
			name:    "warm pool create",
			method:  http.MethodPost,
			path:    pools,
			body:    `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxWarmPool","metadata":{"name":"p"},"spec":{"replicas":1,"sandboxTemplateRef":{"name":"t"}}}`,
			allowed: true,
		},
		{
			name:   "shared ECR pull secret with an image outside the allowlist",
			method: http.MethodPost,
			path:   templates,
			body:   `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"containerDiskImage":"ghcr.io/acme/private:1","imagePullSecret":"ecr-credentials"}}}`,
		},
		{
			name:   "arbitrary pull secret name",
			method: http.MethodPost,
			path:   templates,
			body:   `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"containerDiskImage":"ghcr.io/acme/private:1","imagePullSecret":"my-registry"}}}`,
		},
		{
			name:   "secret read",
			method: http.MethodGet,
			path:   "api/v1/namespaces/ns-a/secrets/ecr-credentials",
		},
		{
			name:   "plain opaque secret create",
			method: http.MethodPost,
			path:   "api/v1/namespaces/ns-a/secrets",
			body:   `{"apiVersion":"v1","kind":"Secret","metadata":{"name":"my-secret"},"stringData":{"a":"b"}}`,
		},
		{
			name:   "dockerconfigjson secret under a non-cua-registry name",
			method: http.MethodPost,
			path:   "api/v1/namespaces/ns-a/secrets",
			body:   `{"apiVersion":"v1","kind":"Secret","type":"kubernetes.io/dockerconfigjson","metadata":{"name":"ecr-credentials","labels":{"cua.ai/registry-secret":"true"}},"data":{".dockerconfigjson":"e30="}}`,
		},
		{
			name:   "delete of the shared ECR pull secret",
			method: http.MethodDelete,
			path:   "api/v1/namespaces/ns-a/secrets/ecr-credentials",
		},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(testCase.method, testCase.path, testCase.body))
			if testCase.allowed {
				if response.Code != http.StatusNoContent || !reached {
					t.Fatalf("status, reached = %d, %v; want %d, true", response.Code, reached, http.StatusNoContent)
				}
				return
			}
			if response.Code != http.StatusForbidden || reached {
				t.Fatalf("status, reached = %d, %v; want %d, false", response.Code, reached, http.StatusForbidden)
			}
		})
	}
}
