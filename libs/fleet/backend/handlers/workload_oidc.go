package handlers

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"net/url"

	"cyclops-cs-backend/identity"
)

// workloadOIDCSecretName is the conventional name of the per-tenant Keycloak
// workload-OIDC credentials Secret written into each pool namespace. The
// OSGym pool's vmTemplate.oidc.credentialsSecret defaults to this (see
// charts/osgym-workspace-test/values.yaml and
// docs/decisions/2026-06-25-osgym-pool-workload-oidc.md).
const workloadOIDCSecretName = "cua-workload-oidc"

// provisionWorkloadOIDC best-effort ensures the tenant's workloads-realm
// Keycloak client exists and writes its credentials into a Secret in the
// just-created namespace, so OSGym pool VMs in that namespace can mint a
// tenant-scoped OIDC token. No-op when the feature is disabled
// (WorkloadAdmin nil). All failures are logged, never surfaced: the
// namespace was created either way, and the OSGym Sandbox controller retries
// until the Secret appears.
func (h Handlers) provisionWorkloadOIDC(ctx context.Context, userSub, namespace string) {
	if h.WorkloadAdmin == nil {
		return
	}
	tenant := identity.PersonalGroup(ctx, userSub)
	clientID, clientSecret, err := h.WorkloadAdmin.EnsureTenantWorkloadClient(
		ctx, userSub, tenant, h.WorkloadAudience,
	)
	if err != nil {
		slog.Warn("workload-oidc: ensure tenant client failed",
			"err", err, "namespace", namespace, "tenant", tenant)
		return
	}
	if err := h.writeWorkloadOIDCSecret(
		ctx, userSub, namespace, clientID, clientSecret, tenant,
	); err != nil {
		slog.Warn("workload-oidc: write credentials Secret failed",
			"err", err, "namespace", namespace)
		return
	}
	slog.Info("workload-oidc: provisioned tenant credentials",
		"namespace", namespace, "tenant", tenant, "client_id", clientID)
}

// writeWorkloadOIDCSecret creates the credentials Secret in the namespace via
// impersonation (the caller has admin in their own Capsule namespace). An
// already-exists conflict is treated as success: EnsureTenantWorkloadClient
// returns a stable secret, so an existing Secret already holds the right
// credentials.
func (h Handlers) writeWorkloadOIDCSecret(
	ctx context.Context, userSub, namespace, clientID, clientSecret, tenant string,
) error {
	secret := map[string]any{
		"apiVersion": "v1",
		"kind":       "Secret",
		"metadata": map[string]any{
			"name":      workloadOIDCSecretName,
			"namespace": namespace,
			"labels": map[string]string{
				"app.kubernetes.io/managed-by": "cyclops-cs-backend",
				"cua.ai/purpose":               "workload-oidc",
			},
		},
		"type": "Opaque",
		// client_id/client_secret are what the in-guest refresher uses;
		// tenant/token_url are surfaced for convenience (a customer reads
		// `tenant` to write their OIDC trust policy condition).
		"stringData": map[string]string{
			"client_id":     clientID,
			"client_secret": clientSecret,
			"tenant":        tenant,
			"token_url":     h.WorkloadTokenURL,
		},
	}
	body, err := json.Marshal(secret)
	if err != nil {
		return err
	}
	resp, err := h.k8sImpersonate(
		"POST",
		"/api/v1/namespaces/"+url.PathEscape(namespace)+"/secrets",
		bytes.NewReader(body), userSub,
	)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	_, _ = io.Copy(io.Discard, resp.Body)
	if resp.StatusCode == http.StatusCreated || resp.StatusCode == http.StatusConflict {
		return nil
	}
	return &k8sStatusError{code: resp.StatusCode}
}

// k8sStatusError is a tiny error carrying an unexpected K8s HTTP status.
type k8sStatusError struct{ code int }

func (e *k8sStatusError) Error() string {
	return "unexpected k8s status " + http.StatusText(e.code)
}
