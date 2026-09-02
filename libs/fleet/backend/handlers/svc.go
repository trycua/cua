package handlers

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log"
	"log/slog"
	"net/http"
	"net/http/httputil"
	"net/url"
	"path"
	"strings"
	"time"

	"cyclops-cs-backend/metrics"
	"cyclops-cs-backend/middlewares"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
)

const statusClientClosedRequest = 499

const oauth2ProxySvcCookieName = "_oauth2_proxy_svc"

func stripOAuth2ProxyIdentityHeaders(header http.Header) {
	for name := range header {
		if strings.HasPrefix(strings.ToLower(name), "x-auth-request-") {
			header.Del(name)
		}
	}
}

func stripOAuth2ProxySvcCookie(header http.Header) {
	var cookies []string
	for _, line := range header.Values("Cookie") {
		for _, cookie := range strings.Split(line, ";") {
			name, _, _ := strings.Cut(strings.TrimSpace(cookie), "=")
			if name != oauth2ProxySvcCookieName {
				cookies = append(cookies, strings.TrimSpace(cookie))
			}
		}
	}
	if len(cookies) == 0 {
		header.Del("Cookie")
		return
	}
	header.Set("Cookie", strings.Join(cookies, "; "))
}

func normalizeProxyUpstreamPath(pathValue string) string {
	trimmed := strings.TrimLeft(pathValue, "/\\")
	if trimmed == "" {
		return "/"
	}
	return "/" + trimmed
}

func svcProxyErrorStatus(err error) int {
	if errors.Is(err, context.Canceled) {
		return statusClientClosedRequest
	}
	return http.StatusBadGateway
}

// rewriteLocation resolves same-origin upstream Location values against the
// current upstream path, then prefixes the proxy base path. External URLs are
// returned unchanged so redirects cannot escape the proxy base.
func rewriteLocation(loc, basePath, upstreamPath string) string {
	if loc == "" {
		return ""
	}
	location, err := url.Parse(loc)
	if err != nil || location.IsAbs() || location.Host != "" {
		return loc
	}
	resolved := (&url.URL{Path: upstreamPath}).ResolveReference(location)
	if hasDotPathSegment(resolved.Path) {
		resolved.Path = path.Clean(resolved.Path)
		resolved.RawPath = ""
	}
	resolved.Path = basePath + resolved.Path
	if resolved.RawPath != "" {
		resolved.RawPath = basePath + resolved.RawPath
	}
	return resolved.String()
}

func hasDotPathSegment(value string) bool {
	for _, segment := range strings.Split(value, "/") {
		if segment == "." || segment == ".." {
			return true
		}
	}
	return false
}

func newSvcReverseProxy(target *url.URL, upstreamPath, basePath string) *httputil.ReverseProxy {
	return newSvcReverseProxyWithRawPath(target, upstreamPath, "", basePath)
}

func newSvcReverseProxyWithRawPath(target *url.URL, upstreamPath, upstreamRawPath, basePath string) *httputil.ReverseProxy {
	rp := httputil.NewSingleHostReverseProxy(target)
	origDirector := rp.Director
	rp.Director = func(req *http.Request) {
		origDirector(req)
		req.URL.Path = upstreamPath
		req.URL.RawPath = upstreamRawPath
		req.Header.Del("Authorization")
		req.Header.Del("Proxy-Authorization")
		stripOAuth2ProxyIdentityHeaders(req.Header)
		stripOAuth2ProxySvcCookie(req.Header)
		req.Host = target.Host
	}
	rp.ModifyResponse = func(resp *http.Response) error {
		loc := resp.Header.Get("Location")
		if rewritten := rewriteLocation(loc, basePath, upstreamPath); rewritten != loc {
			resp.Header.Set("Location", rewritten)
		}
		return nil
	}
	return rp
}

func upstreamRawPath(r *http.Request, basePath, fallback string) string {
	rawPath := r.URL.EscapedPath()
	if strings.HasPrefix(rawPath, "/api/signed-svc/") {
		_, suffix, found := strings.Cut(strings.TrimPrefix(rawPath, "/api/signed-svc/"), "/")
		if !found {
			return "/"
		}
		rawPath = "/" + suffix
	} else {
		if !strings.HasPrefix(rawPath, basePath) {
			return fallback
		}
		rawPath = strings.TrimPrefix(rawPath, basePath)
	}
	if rawPath == "" {
		return "/"
	}
	if !strings.HasPrefix(rawPath, "/") {
		return fallback
	}
	if rawPath == fallback {
		return ""
	}
	return rawPath
}

func (h Handlers) proxyService(w http.ResponseWriter, r *http.Request, namespace, service, upstreamPath, basePath, telemetryKind string) {
	host := fmt.Sprintf("%s.%s.%s", service, namespace, h.GatewayCfg.ClusterDomain)
	target := &url.URL{Scheme: h.GatewayCfg.Scheme, Host: host + ":" + h.GatewayCfg.Port}
	route, _ := r.Context().Value(middlewares.ContextKey("route")).(string)
	if route == "" {
		route = "/api/svc/{namespace}/{service}"
		if r.PathValue("path") != "" {
			route += "/{path...}"
		}
		if telemetryKind == "signed-svc" {
			route = "/api/signed-svc/{token}"
			if r.PathValue("path") != "" {
				route += "/{path...}"
			}
		}
	}
	spanAttrs := []attribute.KeyValue{
		attribute.String("proxy.target", telemetryKind),
		attribute.String("http.route", route),
	}
	if telemetryKind != "signed-svc" {
		spanAttrs = append(spanAttrs,
			attribute.String("k8s.namespace", namespace),
			attribute.String("k8s.service", service),
			attribute.String("http.target_path", upstreamPath),
		)
	}
	ctx, span := handlerTracer().Start(r.Context(), telemetryKind+".proxy", trace.WithAttributes(spanAttrs...))
	defer span.End()

	upstreamRawPath := upstreamRawPath(r, basePath, upstreamPath)
	rp := newSvcReverseProxyWithRawPath(target, upstreamPath, upstreamRawPath, basePath)
	if telemetryKind == "signed-svc" {
		rp.ErrorLog = log.New(io.Discard, "", 0)
	}
	origDirector := rp.Director
	rp.Director = func(req *http.Request) {
		origDirector(req)
		req.URL.RawQuery = r.URL.RawQuery
	}

	rw := &statusCapture{ResponseWriter: w}
	rp.ErrorHandler = func(rw http.ResponseWriter, _ *http.Request, err error) {
		status := svcProxyErrorStatus(err)
		if status == statusClientClosedRequest {
			span.SetStatus(codes.Unset, "client canceled")
			if telemetryKind == "signed-svc" {
				slog.Info("signed-svc proxy canceled")
			} else {
				slog.Info(telemetryKind+" proxy canceled", "host", host, "path", upstreamPath, "err", err)
			}
			rw.WriteHeader(status)
			return
		}
		span.SetStatus(codes.Error, "upstream unavailable")
		if telemetryKind == "signed-svc" {
			slog.Warn("signed-svc proxy error")
		} else {
			span.RecordError(err)
			slog.Warn(telemetryKind+" proxy error", "host", host, "path", upstreamPath, "err", err)
		}
		http.Error(rw, "upstream unavailable", status)
	}

	start := time.Now()
	if telemetryKind == "svc" {
		logAttrs := []any{"namespace", namespace, "service", service, "path", upstreamPath}
		if user := currentUser(r); user != nil {
			logAttrs = append([]any{"client", user.AZP}, logAttrs...)
		}
		slog.Info("svc proxy", logAttrs...)
	}
	rp.ServeHTTP(rw, r.WithContext(ctx))
	span.SetAttributes(attribute.Int("http.status_code", rw.statusCode))
	metrics.RecordUpstreamProxy(telemetryKind, rw.statusCode, time.Since(start))
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

	// Tenancy boundary: this proxy dials Service DNS directly (never the K8s
	// API), so Capsule can't scope it. The boundary is the ownership conjunct
	// of SvcRoutePolicy, which has already run by the time this handler is
	// reached — see auth/authz_ownership.rego.

	upstreamPath := normalizeProxyUpstreamPath(r.PathValue("path"))

	h.proxyService(
		w,
		r,
		ns,
		service,
		upstreamPath,
		fmt.Sprintf("/api/svc/%s/%s", ns, service),
		"svc",
	)

}
