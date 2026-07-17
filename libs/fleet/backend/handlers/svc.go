package handlers

import (
	"fmt"
	"log/slog"
	"net/http"
	"net/http/httputil"
	"net/url"
	"strings"
	"time"

	"cyclops-cs-backend/metrics"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
)

// rewriteLocation prepends basePath to upstream Location header values
// so browsers stay within the /api/svc proxy prefix. External URLs
// (with a host) are returned unchanged.
func rewriteLocation(loc, basePath string) string {
	if loc == "" {
		return ""
	}
	// Absolute path: /foo → /api/svc/{ns}/{svc}/foo
	if strings.HasPrefix(loc, "/") {
		return basePath + loc
	}
	// Full URL with host — leave as-is (external redirect)
	parsed, err := url.Parse(loc)
	if err != nil || parsed.Host != "" {
		return loc
	}
	// Relative path: foo → /api/svc/{ns}/{svc}/foo
	return basePath + "/" + loc
}

// Svc godoc
//
//	@Summary		Authenticated reverse proxy to a K8s Service in a namespace the caller owns
//	@Description	Proxies to {service}.{namespace}.svc.cluster.local:80. Per-key tokens are bound to their `namespace` claim; all other principals (SPA, user keys, oauth2-proxy browser sessions) must hold RBAC in {namespace}, verified via an impersonated RoleBinding probe. Strips Authorization before forwarding.
//	@Tags			gateway
//	@Param			namespace	path		string	true	"K8s namespace (caller must own it, or it must match a per-key token's namespace claim)"
//	@Param			service		path		string	true	"K8s Service name (DNS-1123 label)"
//	@Param			path		path		string	false	"Upstream path (proxied verbatim)"
//	@Success		200			{string}	string	"Upstream response"
//	@Failure		400			{object}	ErrorResponse
//	@Failure		401			{object}	ErrorResponse
//	@Failure		403			{object}	ErrorResponse
//	@Failure		502			{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/svc/{namespace}/{service}/{path} [get]
func (h Handlers) Svc(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}

	ns := r.PathValue("namespace")
	service := r.PathValue("service")
	// Defense in depth — OPA already enforced the DNS-label shape, but
	// validate here too so a policy regression can't turn the proxy into
	// an SSRF vector (same belt-and-braces as Gateway/Orch).
	if !dnsLabel.MatchString(ns) || !dnsLabel.MatchString(service) {
		writeErr(w, http.StatusBadRequest, "invalid namespace or service")
		return
	}

	// Tenancy boundary: this proxy dials Service DNS directly (never the
	// K8s API), so Capsule can't scope it — enforce ownership here.
	if !h.requireNamespaceAccess(w, r, user, ns) {
		return
	}

	host := fmt.Sprintf("%s.%s.%s", service, ns, h.GatewayCfg.ClusterDomain)
	target := &url.URL{Scheme: h.GatewayCfg.Scheme, Host: host + ":" + h.GatewayCfg.Port}

	upstreamPath := r.PathValue("path")
	if upstreamPath == "" {
		upstreamPath = "/"
	} else if !strings.HasPrefix(upstreamPath, "/") {
		upstreamPath = "/" + upstreamPath
	}
	ctx, span := handlerTracer().Start(r.Context(), "svc.proxy", trace.WithAttributes(
		attribute.String("proxy.target", "svc"),
		attribute.String("http.route", "/api/svc/{namespace}/{service}/{path...}"),
		attribute.String("k8s.namespace", ns),
		attribute.String("k8s.service", service),
		attribute.String("http.target_path", upstreamPath),
	))
	defer span.End()

	// The base path prefix that the upstream doesn't know about.
	// Upstream redirects (Location headers) must be rewritten to
	// prepend this prefix so the browser stays within /api/svc.
	basePath := fmt.Sprintf("/api/svc/%s/%s", ns, service)

	rp := httputil.NewSingleHostReverseProxy(target)
	origDirector := rp.Director
	rp.Director = func(req *http.Request) {
		origDirector(req)
		req.URL.Path = upstreamPath
		req.URL.RawPath = ""
		req.URL.RawQuery = r.URL.RawQuery
		req.Header.Del("Authorization")
		req.Host = target.Host
	}
	// Rewrite upstream Location headers to include the base path.
	rp.ModifyResponse = func(resp *http.Response) error {
		loc := resp.Header.Get("Location")
		if rewritten := rewriteLocation(loc, basePath); rewritten != loc {
			resp.Header.Set("Location", rewritten)
		}
		return nil
	}

	rw := &statusCapture{ResponseWriter: w}
	rp.ErrorHandler = func(rw http.ResponseWriter, _ *http.Request, err error) {
		span.RecordError(err)
		span.SetStatus(codes.Error, "upstream unavailable")
		slog.Warn("svc proxy error", "host", host, "path", upstreamPath, "err", err)
		http.Error(rw, "upstream unavailable", http.StatusBadGateway)
	}

	start := time.Now()
	slog.Info("svc proxy",
		"client", user.AZP, "namespace", ns, "service", service, "path", upstreamPath)
	rp.ServeHTTP(rw, r.WithContext(ctx))
	span.SetAttributes(attribute.Int("http.status_code", rw.statusCode))
	metrics.RecordUpstreamProxy("svc", rw.statusCode, time.Since(start))
}
