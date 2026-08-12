package handlers

import (
	"context"
	"encoding/json"
	"net/http"
	"testing"
)

// When the feature is disabled (WorkloadAdmin nil), provisioning is a no-op
// and never touches Kubernetes.
func TestProvisionWorkloadOIDC_DisabledIsNoop(t *testing.T) {
	fk := newFakeK8s(http.StatusCreated, "{}")
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := Handlers{} // WorkloadAdmin == nil
	h.provisionWorkloadOIDC(context.Background(), "sub-1", "mypool")

	if len(fk.requests) != 0 {
		t.Fatalf("disabled feature made %d k8s request(s), want 0", len(fk.requests))
	}
}

func TestWriteWorkloadOIDCSecret_PostsSecret(t *testing.T) {
	fk := newFakeK8s(http.StatusCreated, "{}")
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := Handlers{
		WorkloadTokenURL: "https://auth.cua.ai/realms/workloads/protocol/openid-connect/token",
	}
	err := h.writeWorkloadOIDCSecret(
		context.Background(), "sub-1", "mypool",
		"tenant-sub-1", "secret-xyz", "user-sub-1",
	)
	if err != nil {
		t.Fatalf("writeWorkloadOIDCSecret err = %v", err)
	}
	if len(fk.requests) != 1 {
		t.Fatalf("expected 1 k8s request, got %d", len(fk.requests))
	}
	req := fk.requests[0]
	if req.method != http.MethodPost ||
		req.path != "/api/v1/namespaces/mypool/secrets" {
		t.Fatalf("req = %s %q, want POST /api/v1/namespaces/mypool/secrets",
			req.method, req.path)
	}
	// Scoped to the caller's tenant via impersonation.
	if got := req.headers.Get("Impersonate-User"); got != "oidc:sub-1" {
		t.Fatalf("Impersonate-User = %q, want oidc:sub-1", got)
	}
	var sec struct {
		Metadata struct {
			Name string `json:"name"`
		} `json:"metadata"`
		StringData map[string]string `json:"stringData"`
	}
	if err := json.Unmarshal(req.body, &sec); err != nil {
		t.Fatalf("secret body not JSON: %v", err)
	}
	if sec.Metadata.Name != workloadOIDCSecretName {
		t.Fatalf("secret name = %q, want %q", sec.Metadata.Name, workloadOIDCSecretName)
	}
	if sec.StringData["client_id"] != "tenant-sub-1" ||
		sec.StringData["client_secret"] != "secret-xyz" ||
		sec.StringData["tenant"] != "user-sub-1" {
		t.Fatalf("unexpected stringData: %+v", sec.StringData)
	}
	if sec.StringData["token_url"] != h.WorkloadTokenURL {
		t.Fatalf("token_url = %q, want %q", sec.StringData["token_url"], h.WorkloadTokenURL)
	}
}

// An already-existing Secret (409) is treated as success: the per-tenant
// client secret is stable, so the existing Secret holds the right creds.
func TestWriteWorkloadOIDCSecret_ConflictIsSuccess(t *testing.T) {
	fk := newFakeK8s(http.StatusConflict, "{}")
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := Handlers{}
	if err := h.writeWorkloadOIDCSecret(
		context.Background(), "sub-1", "mypool", "cid", "csec", "user-sub-1",
	); err != nil {
		t.Fatalf("409 should be treated as success, got err = %v", err)
	}
}

func TestWriteWorkloadOIDCSecret_ServerErrorPropagates(t *testing.T) {
	fk := newFakeK8s(http.StatusInternalServerError, "boom")
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := Handlers{}
	if err := h.writeWorkloadOIDCSecret(
		context.Background(), "sub-1", "mypool", "cid", "csec", "user-sub-1",
	); err == nil {
		t.Fatal("expected an error on 500, got nil")
	}
}
