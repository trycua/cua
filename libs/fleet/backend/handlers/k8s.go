package handlers

import (
	"log/slog"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"strings"
	"time"

	"cyclops-cs-backend/identity"
	"cyclops-cs-backend/metrics"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
)

// kubectlProxyAddr is the address of the in-pod kubectl-proxy sidecar.
// kubectl proxy authenticates against the K8s API using the pod's
// ServiceAccount; we keep that hop and just add SPA auth in front of it.
func kubectlProxyAddr() string {
	if v := os.Getenv("KUBECTL_PROXY_ADDR"); v != "" {
		return v
	}
	return "http://127.0.0.1:8001"
}

// K8s godoc
//
//	@Summary		Authenticated proxy to the in-pod kubectl-proxy sidecar
//	@Description	Forwards requests to http://127.0.0.1:8001 (the kubectl-proxy sidecar) so the caller can read K8s resources via the pod ServiceAccount. SPA-only; OPA-gated. The policy is an allowlist: only enumerated group/version/resource/method combinations are proxied (the osgym.cua.ai and cua.ai fleet CRDs, namespaced pod/service reads, a PATCH on osgymsandboxes items whose body touches only spec.vmTemplate.services, pod logs and metrics, KubeVirt reads, and API discovery). Anything else, including Kubernetes events and any cluster-scoped path, is denied and never reaches the sidecar.
//	@Tags			passthrough
//	@Param			path	path		string	true	"K8s API path"
//	@Success		200		{string}	string	"K8s API response"
//	@Failure		401		{object}	ErrorResponse
//	@Failure		403		{object}	ErrorResponse
//	@Failure		502		{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/k8s/{path} [get]
func (h Handlers) K8s(w http.ResponseWriter, r *http.Request) {
	target, err := url.Parse(kubectlProxyAddr())
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "bad KUBECTL_PROXY_ADDR")
		return
	}
	upstreamPath := "/" + strings.TrimPrefix(r.PathValue("path"), "/")
	ctx, span := handlerTracer().Start(r.Context(), "k8s.proxy", trace.WithAttributes(
		attribute.String("proxy.target", "k8s"),
		attribute.String("http.route", "/api/k8s/{path...}"),
		attribute.String("k8s.path", upstreamPath),
	))
	defer span.End()

	// Extract the authenticated user for K8s impersonation — Capsule
	// uses these headers to scope the request to the user's Tenant.
	user := currentUser(r.WithContext(ctx))

	rp := httputil.NewSingleHostReverseProxy(target)
	origDirector := rp.Director
	rp.Director = func(req *http.Request) {
		origDirector(req)
		req.URL.Path = upstreamPath
		req.URL.RawPath = ""
		req.URL.RawQuery = r.URL.RawQuery
		// kubectl-proxy default --accept-hosts rejects requests whose Host
		// header isn't localhost-ish (existing nginx block worked around
		// this the same way).
		req.Host = "localhost"
		req.Header.Set("Host", "localhost")
		req.Header.Del("Authorization")

		// K8s impersonation headers — the kubectl-proxy ServiceAccount
		// needs the "impersonate" verb on users and groups for this to
		// work. The OIDC username format is "oidc:<sub>" and the group
		// is "oidc:user-<sub>", matching the Keycloak OIDC provider
		// config on the K3s apiserver. Capsule intercepts these to
		// enforce per-tenant namespace scoping.
		if user != nil && user.ID != "" {
			ctx := req.Context()
			req.Header.Set("Impersonate-User", identity.ImpersonateUser(ctx, user.ID))
			req.Header.Set("Impersonate-Group", identity.ImpersonateGroup(ctx, user.ID))
		}
	}

	rw := &statusCapture{ResponseWriter: w}
	rp.ErrorHandler = func(rw http.ResponseWriter, _ *http.Request, err error) {
		span.RecordError(err)
		span.SetStatus(codes.Error, "kubectl-proxy unavailable")
		slog.Warn("k8s proxy error", "path", upstreamPath, "err", err)
		http.Error(rw, "kubectl-proxy unavailable", http.StatusBadGateway)
	}
	start := time.Now()
	rp.ServeHTTP(rw, r.WithContext(ctx))
	span.SetAttributes(attribute.Int("http.status_code", rw.statusCode))
	metrics.RecordUpstreamProxy("k8s", rw.statusCode, time.Since(start))
}
