package handlers

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"strings"
	"time"

	"cyclops-cs-backend/auth"
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
//	@Description	Forwards requests to http://127.0.0.1:8001 (the kubectl-proxy sidecar) so the SPA can read K8s resources via the pod ServiceAccount. SPA-only; OPA-gated. EventList responses are filtered by the caller's OPA visible_events policy via auth.K8sEventFilterMiddleware (mounted in main.go's route chain).
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

	if allowed, err := poolImagePullSecretAllowed(ctx, r); err != nil {
		writeErr(w, http.StatusBadRequest, "could not inspect pool request")
		return
	} else if !allowed {
		writeErr(w, http.StatusForbidden, "pool image pull secret or image is not allowed")
		return
	}

	// macOS is an ADMIN-ONLY capability. The SPA hides the option for
	// non-admins, but that is not a boundary — enforce it here so a customer
	// cannot create a macOS pool by crafting the API call directly. Any
	// osgymworkspacepools write whose body requests the macOS runtime requires
	// admin membership; non-macOS pools and all reads are unaffected. CUA-macos.
	if forbid, err := macosPoolNeedsAdmin(ctx, user, r); err != nil {
		writeErr(w, http.StatusBadRequest, "could not inspect pool request")
		return
	} else if forbid {
		writeErr(w, http.StatusForbidden, "macOS pools are restricted to administrators")
		return
	}

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

func poolImagePullSecretAllowed(ctx context.Context, r *http.Request) (bool, error) {
	switch r.Method {
	case http.MethodPost, http.MethodPut, http.MethodPatch:
	default:
		return true, nil
	}
	if !strings.Contains(r.PathValue("path"), "osgymworkspacepools") || r.Body == nil {
		return true, nil
	}
	body, err := io.ReadAll(io.LimitReader(r.Body, 1<<20))
	if err != nil {
		return false, err
	}
	r.Body = io.NopCloser(bytes.NewReader(body))
	var object map[string]any
	if err := json.Unmarshal(body, &object); err != nil {
		return false, err
	}
	return auth.EvalPoolAdmission(ctx, r.Method, object)
}

// macosPoolNeedsAdmin returns true if r is a write to an osgymworkspacepools
// resource whose body opts into the macOS runtime AND the caller is not an
// admin. It rebuffers r.Body so the downstream proxy can still read it. Reads
// and non-macOS writes always return false (no restriction).
func macosPoolNeedsAdmin(ctx context.Context, user *auth.User, r *http.Request) (bool, error) {
	switch r.Method {
	case http.MethodPost, http.MethodPut, http.MethodPatch:
	default:
		return false, nil
	}
	if !strings.Contains(r.PathValue("path"), "osgymworkspacepools") || r.Body == nil {
		return false, nil
	}
	body, err := io.ReadAll(io.LimitReader(r.Body, 1<<20))
	if err != nil {
		return false, err
	}
	r.Body = io.NopCloser(bytes.NewReader(body)) // rebuffer for the proxy
	if !bodyRequestsMacOS(body) {
		return false, nil
	}
	isAdmin, err := auth.EvalIsAdmin(ctx, user)
	if err != nil {
		return false, err
	}
	return !isAdmin, nil
}

// bodyRequestsMacOS reports whether an OSGymWorkspacePool CR body opts into the
// macOS runtime — via spec.template.runtime=macos, or (defence in depth) the
// macOS-only runtimeClass / nodeSelector that a macOS pod carries.
func bodyRequestsMacOS(body []byte) bool {
	var obj map[string]any
	if json.Unmarshal(body, &obj) != nil {
		return false
	}
	spec, _ := obj["spec"].(map[string]any)
	tmpl, _ := spec["template"].(map[string]any)
	if tmpl == nil {
		return false
	}
	if s, _ := tmpl["runtime"].(string); strings.EqualFold(s, "macos") {
		return true
	}
	if s, _ := tmpl["runtimeClassName"].(string); s == "cua-macos-native" {
		return true
	}
	if ns, _ := tmpl["nodeSelector"].(map[string]any); ns != nil {
		if _, ok := ns["cua.ai/macos"]; ok {
			return true
		}
	}
	return false
}

// Orch godoc
//
//	@Summary		SPA-authenticated proxy to a per-namespace orchestrator service
//	@Description	Resolves <service>.<namespace>.svc.cluster.local at request time (in-cluster DNS). The caller must hold RBAC in {namespace} (verified via an impersonated RoleBinding probe); OPA additionally validates that namespace and service look like DNS-1123 labels.
//	@Tags			passthrough
//	@Param			namespace	path		string	true	"Namespace (DNS-1123 label)"
//	@Param			service		path		string	true	"Service name (DNS-1123 label)"
//	@Param			path		path		string	true	"Upstream path"
//	@Success		200			{string}	string	"Upstream response"
//	@Failure		400			{object}	ErrorResponse
//	@Failure		401			{object}	ErrorResponse
//	@Failure		403			{object}	ErrorResponse
//	@Failure		502			{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/orch/{namespace}/{service}/{path} [get]
func (h Handlers) Orch(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	ns := r.PathValue("namespace")
	svc := r.PathValue("service")
	if !dnsLabel.MatchString(ns) || !dnsLabel.MatchString(svc) {
		writeErr(w, http.StatusBadRequest, "invalid namespace or service")
		return
	}
	// Tenancy boundary: this proxy dials Service DNS directly (never the
	// K8s API), so Capsule can't scope it — enforce ownership here.
	if !h.requireNamespaceAccess(w, r, user, ns) {
		return
	}
	upstreamPath := "/" + strings.TrimPrefix(r.PathValue("path"), "/")
	host := svc + "." + ns + "." + h.GatewayCfg.ClusterDomain
	target := &url.URL{Scheme: h.GatewayCfg.Scheme, Host: host + ":" + h.GatewayCfg.Port}
	ctx, span := handlerTracer().Start(r.Context(), "orch.proxy", trace.WithAttributes(
		attribute.String("proxy.target", "orch"),
		attribute.String("http.route", "/api/orch/{namespace}/{service}/{path...}"),
		attribute.String("k8s.namespace", ns),
		attribute.String("k8s.service", svc),
		attribute.String("http.target_path", upstreamPath),
	))
	defer span.End()

	rp := httputil.NewSingleHostReverseProxy(target)
	origDirector := rp.Director
	rp.Director = func(req *http.Request) {
		origDirector(req)
		req.URL.Path = upstreamPath
		req.URL.RawPath = ""
		req.URL.RawQuery = r.URL.RawQuery
		req.Host = target.Host
		req.Header.Del("Authorization")
	}
	rw := &statusCapture{ResponseWriter: w}
	rp.ErrorHandler = func(rw http.ResponseWriter, _ *http.Request, err error) {
		span.RecordError(err)
		span.SetStatus(codes.Error, "upstream unavailable")
		slog.Warn("orch proxy error", "host", host, "path", upstreamPath, "err", err)
		http.Error(rw, "upstream unavailable", http.StatusBadGateway)
	}
	start := time.Now()
	rp.ServeHTTP(rw, r.WithContext(ctx))
	span.SetAttributes(attribute.Int("http.status_code", rw.statusCode))
	metrics.RecordUpstreamProxy("orch", rw.statusCode, time.Since(start))
}
