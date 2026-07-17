package handlers

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync"
	"time"

	"cyclops-cs-backend/identity"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
)

// ── Request / Response types ───────────────────────────────────────────

// CreateNamespaceRequest is the body for POST /api/namespaces.
type CreateNamespaceRequest struct {
	Name string `json:"name" example:"my-workspace"`
}

// NamespaceResponse is a single namespace entry returned by the API.
type NamespaceResponse struct {
	Name      string            `json:"name"`
	Status    string            `json:"status"`
	CreatedAt string            `json:"createdAt"`
	Labels    map[string]string `json:"labels"`
}

// ── Impersonation helper ───────────────────────────────────────────────

// k8sImpersonate performs a K8s API request on behalf of the authenticated
// user. Production routes this through Capsule Proxy for Tenant filtering;
// environments without Capsule Proxy fall back to the direct apiserver.
//
// The OIDC username format is "oidc:<sub>" and the shared Capsule gate group
// is "oidc:cyclops-cs-tenants", matching the K3s OIDC configuration.

var (
	k8sClientOnce sync.Once
	k8sClient     *http.Client
	k8sAPIServer  string
	// k8sDirectAPIServer always targets the in-cluster apiserver. The
	// impersonated path may instead use Capsule Proxy for tenant filtering.
	k8sDirectAPIServer string
	k8sSAToken         string
)

// overrideK8sClient is used by tests to inject a fake K8s API server.
// It sets the client AND marks the Once as done so initK8sClient() is a no-op.
func overrideK8sClient(client *http.Client, apiServer, saToken string) {
	overrideK8sClientTargets(client, apiServer, apiServer, saToken)
}

func overrideK8sClientTargets(
	client *http.Client, impersonatedServer, directServer, saToken string,
) {
	k8sClientOnce.Do(func() {}) // exhaust the Once
	k8sClient = client
	k8sAPIServer = impersonatedServer
	k8sDirectAPIServer = directServer
	k8sSAToken = saToken
}

func initK8sClient() {
	k8sClientOnce.Do(func() {
		// Direct in-cluster apiserver address. ServiceAccount-only control-plane
		// operations must not depend on Capsule Proxy behavior.
		host := os.Getenv("KUBERNETES_SERVICE_HOST")
		port := os.Getenv("KUBERNETES_SERVICE_PORT")
		if host == "" {
			host = "kubernetes.default.svc"
		}
		if port == "" {
			port = "443"
		}
		if strings.Contains(host, ":") {
			host = "[" + host + "]"
		}
		k8sDirectAPIServer = fmt.Sprintf("https://%s:%s", host, port)

		// Capsule Proxy — filters list responses by Tenant ownership.
		// Falls back to direct K8s API if CAPSULE_PROXY_ADDR is not set.
		if proxyAddr := os.Getenv("CAPSULE_PROXY_ADDR"); proxyAddr != "" {
			k8sAPIServer = proxyAddr
			slog.Info("k8sImpersonate: using Capsule Proxy", "addr", proxyAddr)
		} else {
			k8sAPIServer = k8sDirectAPIServer
			slog.Info("k8sImpersonate: using K8s API directly", "addr", k8sAPIServer)
		}

		// Load SA token
		tokenBytes, err := os.ReadFile("/var/run/secrets/kubernetes.io/serviceaccount/token")
		if err != nil {
			slog.Warn("k8sImpersonate: cannot read SA token", "err", err)
			return
		}
		k8sSAToken = strings.TrimSpace(string(tokenBytes))

		// Load CA cert
		caCert, err := os.ReadFile("/var/run/secrets/kubernetes.io/serviceaccount/ca.crt")
		if err != nil {
			slog.Warn("k8sImpersonate: cannot read CA cert, using system pool", "err", err)
			k8sClient = &http.Client{Timeout: 15 * time.Second}
			return
		}
		pool := x509.NewCertPool()
		pool.AppendCertsFromPEM(caCert)
		k8sClient = &http.Client{
			Timeout: 15 * time.Second,
			Transport: &http.Transport{
				TLSClientConfig: &tls.Config{RootCAs: pool},
			},
		}
	})
}

func (h Handlers) k8sImpersonate(
	ctx context.Context, method, path string, body io.Reader, userSub string,
) (*http.Response, error) {
	if k8sClient == nil {
		initK8sClient()
	}
	if k8sClient == nil {
		return nil, fmt.Errorf("k8s client not initialised (missing SA token)")
	}

	target := k8sAPIServer + path
	req, err := http.NewRequestWithContext(ctx, method, target, body)
	if err != nil {
		return nil, fmt.Errorf("build request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	// Authenticate as the pod's ServiceAccount
	req.Header.Set("Authorization", "Bearer "+k8sSAToken)

	// K8s impersonation headers — the SA needs the "impersonate" verb
	// on users and groups (granted by cyclops-cs-impersonator ClusterRole).
	// Prefixes resolve from OpenFeature (see package identity) so Tenant
	// provisioning, impersonation, and apiserver flags stay aligned.
	req.Header.Set("Impersonate-User", identity.ImpersonateUser(ctx, userSub))
	req.Header.Set("Impersonate-Group", identity.ImpersonateGroup(ctx, userSub))

	return k8sClient.Do(req)
}

// Adoption-wait tuning. Capsule normally adopts within a second or two; the
// cap just keeps a wedged Capsule controller from stalling namespace creation.
const (
	adoptionWaitTimeout = 10 * time.Second
	adoptionWaitStep    = 500 * time.Millisecond
)

// waitForNamespaceAdoption polls a RoleBinding LIST in the new namespace as
// the impersonated user until it succeeds (Capsule's owner RoleBindings are in
// place) or the timeout elapses. RoleBinding list is the probe because it is
// only authorized by the `admin` RoleBinding Capsule creates on adoption — the
// very thing being waited for. (A GET on the namespace itself does NOT work:
// the capsule-tenant-cluster-resources ClusterRole grants namespaces get/list
// cluster-wide to system:authenticated, so that probe succeeds instantly and
// gates nothing.) Best-effort by design: failures are logged, never
// surfaced — the namespace was created either way.
func (h Handlers) waitForNamespaceAdoption(ctx context.Context, name, userSub string) {
	adoptionCtx, cancel := context.WithTimeout(ctx, adoptionWaitTimeout)
	defer cancel()
	for {
		resp, err := h.k8sImpersonate(adoptionCtx, "GET",
			"/apis/rbac.authorization.k8s.io/v1/namespaces/"+url.PathEscape(name)+"/rolebindings?limit=1",
			nil, userSub)
		if err == nil {
			_, _ = io.Copy(io.Discard, resp.Body)
			resp.Body.Close()
			if resp.StatusCode >= 200 && resp.StatusCode < 300 {
				return
			}
		}
		if adoptionCtx.Err() != nil {
			slog.Warn("namespace create: adoption wait timed out; returning anyway",
				"namespace", name)
			return
		}
		select {
		case <-time.After(adoptionWaitStep):
		case <-adoptionCtx.Done():
			return
		}
	}
}

// ── Handlers ───────────────────────────────────────────────────────────

// ListNamespaces godoc
//
//	@Summary		List the calling user's namespaces
//	@Description	Returns namespaces owned by the caller's Capsule Tenant. The list is scoped by a capsule.clastix.io/tenant=<tenant> label selector built from the authenticated subject, so it stays fail-closed even when Capsule Proxy isn't filtering.
//	@Tags			namespaces
//	@Produce		json
//	@Success		200	{array}		NamespaceResponse
//	@Failure		401	{object}	ErrorResponse
//	@Failure		502	{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/namespaces [get]
func (h Handlers) ListNamespaces(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	ctx, span := handlerTracer().Start(r.Context(), "namespaces.list", trace.WithAttributes(
		attribute.String("http.route", "/api/namespaces"),
		attribute.String("user.id", user.ID),
	))
	defer span.End()

	// Scope the list to the caller's own Tenant. Every namespace owned by a
	// Tenant carries capsule.clastix.io/tenant=<tenant> (set on create — see
	// CreateNamespace — and enforced by Capsule). Selecting on it makes this
	// fail-closed: even if Capsule Proxy isn't filtering (e.g. while
	// CapsuleConfiguration.userGroups is being reconciled, or right after a
	// capsule-proxy restart with a cold cache), the apiserver only returns the
	// caller's namespaces. The personal Tenant name follows the "user-<sub>"
	// convention, and the selector is derived from the authenticated subject,
	// so a caller can never widen it to another tenant.
	selector := "capsule.clastix.io/tenant=" + identity.PersonalGroup(ctx, user.ID)
	path := "/api/v1/namespaces?labelSelector=" + url.QueryEscape(selector)
	resp, err := h.k8sImpersonate(ctx, "GET", path, nil, user.ID)
	if err != nil {
		span.RecordError(err)
		span.SetStatus(codes.Error, "namespace list request failed")
		slog.Warn("namespace list: k8s request failed", "err", err)
		writeErr(w, http.StatusBadGateway, "kubectl-proxy unavailable")
		return
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		span.SetStatus(codes.Error, "namespace list returned non-200")
		span.SetAttributes(attribute.Int("http.status_code", resp.StatusCode))
		slog.Warn("namespace list: k8s error", "status", resp.StatusCode, "body", string(body))
		writeErr(w, resp.StatusCode, "k8s: "+string(body))
		return
	}

	// Parse K8s NamespaceList.
	var nsList struct {
		Items []struct {
			Metadata struct {
				Name              string            `json:"name"`
				Labels            map[string]string `json:"labels"`
				CreationTimestamp string            `json:"creationTimestamp"`
			} `json:"metadata"`
			Status struct {
				Phase string `json:"phase"`
			} `json:"status"`
		} `json:"items"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&nsList); err != nil {
		span.RecordError(err)
		span.SetStatus(codes.Error, "namespace list decode failed")
		slog.Warn("namespace list: decode error", "err", err)
		writeErr(w, http.StatusBadGateway, "bad response from k8s")
		return
	}

	out := make([]NamespaceResponse, 0, len(nsList.Items))
	for _, ns := range nsList.Items {
		out = append(out, NamespaceResponse{
			Name:      ns.Metadata.Name,
			Status:    ns.Status.Phase,
			CreatedAt: ns.Metadata.CreationTimestamp,
			Labels:    ns.Metadata.Labels,
		})
	}
	span.SetAttributes(
		attribute.Int("http.status_code", http.StatusOK),
		attribute.Int("namespaces.count", len(out)),
	)
	writeJSON(w, http.StatusOK, out)
}

// CreateNamespace godoc
//
//	@Summary		Create a namespace for the calling user
//	@Description	Creates a K8s namespace via impersonation. Capsule's webhook intercepts the creation and assigns it to the user's Tenant.
//	@Tags			namespaces
//	@Accept			json
//	@Produce		json
//	@Param			body	body		CreateNamespaceRequest	true	"Namespace parameters"
//	@Success		201		{object}	NamespaceResponse
//	@Failure		400		{object}	ErrorResponse
//	@Failure		401		{object}	ErrorResponse
//	@Failure		409		{object}	ErrorResponse
//	@Failure		502		{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/namespaces [post]
func (h Handlers) CreateNamespace(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil || user.ID == "" {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}

	var req CreateNamespaceRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid body")
		return
	}
	if req.Name == "" {
		writeErr(w, http.StatusBadRequest, "name is required")
		return
	}
	if !dnsLabel.MatchString(req.Name) || len(req.Name) > 63 {
		writeErr(w, http.StatusBadRequest, "name must be a DNS-1123 label (lowercase alphanumeric + dashes, max 63 chars)")
		return
	}
	ctx, span := handlerTracer().Start(r.Context(), "namespaces.create", trace.WithAttributes(
		attribute.String("http.route", "/api/namespaces"),
		attribute.String("user.id", user.ID),
		attribute.String("k8s.namespace", req.Name),
	))
	defer span.End()

	// Tenant provisioning is just-in-time so a freshly authenticated user can
	// create their first namespace without waiting for a separate reconciler.
	// This request runs as the backend ServiceAccount (not as the user) and
	// fails closed if an existing Tenant has unexpected ownership or roles.
	tenantName, err := h.ensurePersonalTenant(ctx, user.ID)
	if err != nil {
		span.RecordError(err)
		span.SetStatus(codes.Error, "personal tenant ensure failed")
		slog.Warn("namespace create: personal tenant ensure failed", "err", err)
		writeErr(w, http.StatusBadGateway, "could not ensure personal tenant")
		return
	}

	// Build K8s Namespace object with Capsule tenant label.
	// The label tells Capsule which Tenant owns this namespace.
	// The prefix resolves from OpenFeature through the same identity helper used
	// above, so the Tenant and namespace label cannot drift.
	nsObj := map[string]interface{}{
		"apiVersion": "v1",
		"kind":       "Namespace",
		"metadata": map[string]interface{}{
			"name": req.Name,
			"labels": map[string]string{
				"capsule.clastix.io/tenant": tenantName,
			},
		},
	}
	body, err := json.Marshal(nsObj)
	if err != nil {
		span.RecordError(err)
		span.SetStatus(codes.Error, "namespace create marshal failed")
		writeErr(w, http.StatusInternalServerError, "marshal error")
		return
	}

	// Capsule's admission webhook observes Tenants through an informer cache.
	// Immediately after first-use Tenant creation, that cache can briefly lag
	// both the object and its status.owners update. Retry only the exact Capsule
	// missing-Tenant 500 and non-owned-Tenant 400; every other transport or
	// admission failure is returned immediately.
	retryCtx, cancelRetry := context.WithTimeout(ctx, tenantInformerWaitTimeout)
	defer cancelRetry()
	var respStatus int
	var respBody []byte
	for {
		resp, requestErr := h.k8sImpersonate(
			retryCtx, "POST", "/api/v1/namespaces", bytes.NewReader(body), user.ID,
		)
		if requestErr != nil {
			span.RecordError(requestErr)
			span.SetStatus(codes.Error, "namespace create request failed")
			slog.Warn("namespace create: k8s request failed", "err", requestErr)
			writeErr(w, http.StatusBadGateway, "kubectl-proxy unavailable")
			return
		}
		respStatus = resp.StatusCode
		respBody, err = readBoundedK8sBody(resp.Body)
		resp.Body.Close()
		if err != nil {
			span.RecordError(err)
			span.SetStatus(codes.Error, "namespace create response failed")
			writeErr(w, http.StatusBadGateway, "bad response from k8s")
			return
		}
		if !isPersonalTenantAdmissionCacheLag(respStatus, respBody, tenantName) {
			break
		}
		select {
		case <-time.After(tenantInformerWaitStep):
		case <-retryCtx.Done():
			span.RecordError(retryCtx.Err())
			span.SetStatus(codes.Error, "namespace create cancelled during tenant readiness wait")
			writeErr(w, http.StatusBadGateway, "namespace create cancelled")
			return
		}
	}

	if respStatus == http.StatusConflict {
		span.SetAttributes(attribute.Int("http.status_code", http.StatusConflict))
		writeErr(w, http.StatusConflict, "namespace already exists")
		return
	}
	if respStatus != http.StatusCreated {
		span.SetStatus(codes.Error, "namespace create returned non-201")
		span.SetAttributes(attribute.Int("http.status_code", respStatus))
		slog.Warn("namespace create: k8s error", "status", respStatus, "body", string(respBody))
		writeErr(w, respStatus, "k8s: "+string(respBody))
		return
	}

	// Wait for Capsule to adopt the namespace before returning, so callers can
	// immediately create resources in it. Adoption is asynchronous: Capsule's
	// controller sets the Tenant ownerReference and creates the owner
	// RoleBindings a moment after the namespace exists. Until those bindings
	// land, the user has NO permissions inside the namespace and an immediate
	// follow-up (e.g. the dashboard's pool create) races into a 403. A GET on
	// the namespace as the impersonated user is a zero-coupling adoption probe:
	// it only succeeds once the RoleBindings exist — the same instant any other
	// in-namespace call becomes authorized. Best-effort: on timeout we still
	// return 201 (the namespace exists; adoption is merely late).
	h.waitForNamespaceAdoption(ctx, req.Name, user.ID)

	// Best-effort: provision the tenant's workload-OIDC credentials into the
	// new namespace so OSGym pool VMs here can mint a tenant-scoped OIDC
	// token (see provisionWorkloadOIDC; no-op when the feature is disabled).
	h.provisionWorkloadOIDC(ctx, user.ID, req.Name)

	// Parse the created namespace.
	var created struct {
		Metadata struct {
			Name              string            `json:"name"`
			Labels            map[string]string `json:"labels"`
			CreationTimestamp string            `json:"creationTimestamp"`
		} `json:"metadata"`
		Status struct {
			Phase string `json:"phase"`
		} `json:"status"`
	}
	if err := json.Unmarshal(respBody, &created); err != nil {
		span.SetAttributes(attribute.Int("http.status_code", http.StatusCreated))
		// Creation succeeded but response parsing failed — still 201.
		writeJSON(w, http.StatusCreated, NamespaceResponse{Name: req.Name, Status: "Active"})
		return
	}
	span.SetAttributes(attribute.Int("http.status_code", http.StatusCreated))
	writeJSON(w, http.StatusCreated, NamespaceResponse{
		Name:      created.Metadata.Name,
		Status:    created.Status.Phase,
		CreatedAt: created.Metadata.CreationTimestamp,
		Labels:    created.Metadata.Labels,
	})
}

// DeleteNamespace godoc
//
//	@Summary		Delete a namespace owned by the calling user
//	@Description	Deletes a K8s namespace via impersonation. Capsule blocks deletion if the namespace doesn't belong to the user's Tenant.
//	@Tags			namespaces
//	@Param			name	path	string	true	"Namespace name"
//	@Success		204
//	@Failure		400		{object}	ErrorResponse
//	@Failure		401		{object}	ErrorResponse
//	@Failure		403		{object}	ErrorResponse
//	@Failure		404		{object}	ErrorResponse
//	@Failure		502		{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/namespaces/{name} [delete]
func (h Handlers) DeleteNamespace(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}

	name := r.PathValue("name")
	if name == "" {
		writeErr(w, http.StatusBadRequest, "name is required")
		return
	}
	if !dnsLabel.MatchString(name) || len(name) > 63 {
		writeErr(w, http.StatusBadRequest, "invalid namespace name")
		return
	}
	ctx, span := handlerTracer().Start(r.Context(), "namespaces.delete", trace.WithAttributes(
		attribute.String("http.route", "/api/namespaces/{name}"),
		attribute.String("user.id", user.ID),
		attribute.String("k8s.namespace", name),
	))
	defer span.End()

	resp, err := h.k8sImpersonate(ctx, "DELETE", "/api/v1/namespaces/"+name, nil, user.ID)
	if err != nil {
		span.RecordError(err)
		span.SetStatus(codes.Error, "namespace delete request failed")
		slog.Warn("namespace delete: k8s request failed", "err", err)
		writeErr(w, http.StatusBadGateway, "kubectl-proxy unavailable")
		return
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		span.SetAttributes(attribute.Int("http.status_code", http.StatusNotFound))
		writeErr(w, http.StatusNotFound, "namespace not found")
		return
	}
	if resp.StatusCode == http.StatusForbidden {
		span.SetAttributes(attribute.Int("http.status_code", http.StatusForbidden))
		writeErr(w, http.StatusForbidden, "not allowed to delete this namespace")
		return
	}
	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusAccepted {
		body, _ := io.ReadAll(resp.Body)
		span.SetStatus(codes.Error, "namespace delete returned unexpected status")
		span.SetAttributes(attribute.Int("http.status_code", resp.StatusCode))
		slog.Warn("namespace delete: k8s error", "status", resp.StatusCode, "body", string(body))
		writeErr(w, resp.StatusCode, "k8s: "+string(body))
		return
	}
	span.SetAttributes(attribute.Int("http.status_code", http.StatusNoContent))
	w.WriteHeader(http.StatusNoContent)
}
