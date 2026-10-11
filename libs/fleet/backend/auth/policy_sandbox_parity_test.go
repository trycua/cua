package auth

import (
	"net/http"
	"testing"
)

// These pin the sandbox-parity surfaces end to end through the production
// tree: tenant registry pull Secrets (create/delete only, dockerconfigjson
// cua-registry-* only), templates pairing them with any image, and the
// env/args/processMode rules with their own 403 message.

const (
	registrySecretCollection = "api/v1/namespaces/ns-a/secrets"
	templateCollection       = "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates"
)

func TestK8sRegistrySecretCreateAndDeleteAdmitted(t *testing.T) {
	setCardAdmissionFlags(t, false)
	const body = `{"apiVersion":"v1","kind":"Secret","type":"kubernetes.io/dockerconfigjson","metadata":{"name":"cua-registry-ghcr","namespace":"ns-a","labels":{"cua.ai/registry-secret":"true"}},"data":{".dockerconfigjson":"eyJhdXRocyI6e319"}}`

	for _, testCase := range []struct {
		name   string
		method string
		path   string
		body   string
	}{
		{name: "create", method: http.MethodPost, path: registrySecretCollection, body: body},
		{name: "delete", method: http.MethodDelete, path: registrySecretCollection + "/cua-registry-ghcr"},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(testCase.method, testCase.path, testCase.body))
			if response.Code != http.StatusNoContent || !reached {
				t.Fatalf("status, reached = %d, %v; want %d, true", response.Code, reached, http.StatusNoContent)
			}
		})
	}
}

func TestK8sNonRegistrySecretCreateReturnsRestrictedMessage(t *testing.T) {
	setCardAdmissionFlags(t, false)
	for _, testCase := range []struct {
		name string
		body string
	}{
		{name: "opaque", body: `{"type":"Opaque","metadata":{"name":"cua-registry-x"},"stringData":{"a":"b"}}`},
		{name: "type omitted", body: `{"metadata":{"name":"cua-registry-x"},"data":{".dockerconfigjson":"e30="}}`},
		{name: "other name", body: `{"type":"kubernetes.io/dockerconfigjson","metadata":{"name":"ecr-credentials"},"data":{".dockerconfigjson":"e30="}}`},
		{name: "service account token", body: `{"type":"kubernetes.io/service-account-token","metadata":{"name":"cua-registry-x","annotations":{"kubernetes.io/service-account.name":"default"}}}`},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(http.MethodPost, registrySecretCollection, testCase.body))
			if response.Code != http.StatusForbidden || reached {
				t.Fatalf("status, reached = %d, %v; want %d, false", response.Code, reached, http.StatusForbidden)
			}
			if got := policyErrorMessage(t, response); got != TenantSecretRestrictedMessage {
				t.Fatalf("error = %q, want %q", got, TenantSecretRestrictedMessage)
			}
		})
	}
}

func TestK8sOtherSecretOperationsStayDenied(t *testing.T) {
	setCardAdmissionFlags(t, false)
	for _, testCase := range []struct {
		method string
		path   string
	}{
		{method: http.MethodGet, path: registrySecretCollection},
		{method: http.MethodGet, path: registrySecretCollection + "/cua-registry-ghcr"},
		{method: http.MethodPatch, path: registrySecretCollection + "/cua-registry-ghcr"},
		{method: http.MethodPut, path: registrySecretCollection + "/cua-registry-ghcr"},
		{method: http.MethodDelete, path: registrySecretCollection + "/ecr-credentials"},
		{method: http.MethodDelete, path: registrySecretCollection + "/workload-oidc"},
	} {
		t.Run(testCase.method+" "+testCase.path, func(t *testing.T) {
			response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(testCase.method, testCase.path, `{"data":{".dockerconfigjson":"e30="}}`))
			if response.Code != http.StatusForbidden || reached {
				t.Fatalf("status, reached = %d, %v; want %d, false", response.Code, reached, http.StatusForbidden)
			}
		})
	}
}

func TestK8sTemplateProcessFieldsAndRegistrySecrets(t *testing.T) {
	setCardAdmissionFlags(t, false)
	for _, testCase := range []struct {
		name    string
		body    string
		allowed bool
		message string
	}{
		{
			name:    "canonical public image, no secret",
			body:    `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"containerDiskImage":"ghcr.io/trycua/linux:24.04","runtime":"gvisor"}}}`,
			allowed: true,
		},
		{
			name:    "private image with tenant registry secret, command, env",
			body:    `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"containerDiskImage":"ghcr.io/acme/agent:1","imagePullSecret":"cua-registry-ghcr","runtime":"gvisor","command":["python","-m","server"],"env":{"FOO":"bar"}}}}`,
			allowed: true,
		},
		{
			name:    "kubevirt command (pre-existing field, still ignored on kubevirt)",
			body:    `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"containerDiskImage":"ghcr.io/trycua/linux:24.04-disk","command":["/usr/bin/python3","-m","http.server"]}}}`,
			allowed: true,
		},
		{
			name:    "kubevirt processMode Run with command, args and env",
			body:    `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"containerDiskImage":"ghcr.io/trycua/linux:24.04-disk","processMode":"Run","command":["python3","-m","http.server"],"args":["$(PORT)"],"env":{"PORT":"8765"},"services":[{"name":"http","targetPort":8765}]}}}`,
			allowed: true,
		},
		{
			name:    "kubevirt processMode Run with a multi-line env value",
			body:    `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"containerDiskImage":"ghcr.io/trycua/linux:24.04-disk","processMode":"Run","env":{"A":"x\ny"}}}}`,
			message: SandboxProcessRestrictedMessage,
		},
		{
			name:    "env on kubevirt with processMode Legacy",
			body:    `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"containerDiskImage":"ghcr.io/trycua/linux:24.04-disk","processMode":"Legacy","env":{"FOO":"bar"}}}}`,
			message: SandboxProcessRestrictedMessage,
		},
		{
			name:    "env on kubevirt",
			body:    `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"containerDiskImage":"ghcr.io/trycua/linux:24.04-disk","env":{"FOO":"bar"}}}}`,
			message: SandboxProcessRestrictedMessage,
		},
		{
			name:    "bad env name",
			body:    `{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxTemplate","metadata":{"name":"t"},"spec":{"vmTemplate":{"containerDiskImage":"redis:7","runtime":"gvisor","env":{"1BAD":"x"}}}}`,
			message: SandboxProcessRestrictedMessage,
		},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			response, reached := runK8sPolicyResponse(t, sandboxServicesRequest(http.MethodPost, templateCollection, testCase.body))
			if testCase.allowed {
				if response.Code != http.StatusNoContent || !reached {
					t.Fatalf("status, reached = %d, %v; want %d, true", response.Code, reached, http.StatusNoContent)
				}
				return
			}
			if response.Code != http.StatusForbidden || reached {
				t.Fatalf("status, reached = %d, %v; want %d, false", response.Code, reached, http.StatusForbidden)
			}
			if got := policyErrorMessage(t, response); got != testCase.message {
				t.Fatalf("error = %q, want %q", got, testCase.message)
			}
		})
	}
}
