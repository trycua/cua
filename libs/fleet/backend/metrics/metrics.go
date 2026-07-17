// Package metrics provides Prometheus metrics for the cyclops-cs backend.
//
// Exported metrics:
//
//	cyclops_cs_http_requests_total          - counter, labels: method, path, status_code
//	cyclops_cs_http_request_duration_seconds - histogram, labels: method, path, status_code
//	cyclops_cs_keycloak_requests_total       - counter, labels: operation, status
//	cyclops_cs_keycloak_request_duration_seconds - histogram, labels: operation, status
//	cyclops_cs_upstream_proxy_requests_total - counter, labels: target, status_code
//	cyclops_cs_upstream_proxy_duration_seconds - histogram, labels: target, status_code
//	cyclops_cs_active_requests              - gauge
//
// The metrics server is started separately on METRICS_ADDR (default :9091)
// so that the main HTTP server on :8080 stays free of /metrics traffic.
package metrics

import (
	"bufio"
	"fmt"
	"net"
	"net/http"
	"strconv"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
	"github.com/prometheus/client_golang/prometheus/promhttp"
)

var (
	// HTTP SLIs
	HTTPRequestsTotal = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "cyclops_cs_http_requests_total",
		Help: "Total HTTP requests handled by the cyclops-cs backend, by method, normalised path, and status code.",
	}, []string{"method", "path", "status_code"})

	HTTPRequestDuration = promauto.NewHistogramVec(prometheus.HistogramOpts{
		Name:    "cyclops_cs_http_request_duration_seconds",
		Help:    "Latency of HTTP requests handled by the cyclops-cs backend.",
		Buckets: []float64{0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10},
	}, []string{"method", "path", "status_code"})

	ActiveRequests = promauto.NewGauge(prometheus.GaugeOpts{
		Name: "cyclops_cs_active_requests",
		Help: "Number of HTTP requests currently being processed by the cyclops-cs backend.",
	})

	// Keycloak admin API SLIs
	KeycloakRequestsTotal = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "cyclops_cs_keycloak_requests_total",
		Help: "Total Keycloak admin API requests made by the cyclops-cs backend.",
	}, []string{"operation", "status"})

	KeycloakRequestDuration = promauto.NewHistogramVec(prometheus.HistogramOpts{
		Name:    "cyclops_cs_keycloak_request_duration_seconds",
		Help:    "Latency of Keycloak admin API requests.",
		Buckets: []float64{0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5},
	}, []string{"operation", "status"})

	// Upstream proxy SLIs (/api/gateway + /api/orch + /api/k8s)
	UpstreamProxyRequestsTotal = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "cyclops_cs_upstream_proxy_requests_total",
		Help: "Total requests proxied to upstream services (gateway/orch/k8s).",
	}, []string{"target", "status_code"})

	UpstreamProxyDuration = promauto.NewHistogramVec(prometheus.HistogramOpts{
		Name:    "cyclops_cs_upstream_proxy_duration_seconds",
		Help:    "Latency of requests proxied to upstream services.",
		Buckets: []float64{0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30},
	}, []string{"target", "status_code"})
)

// responseWriter wraps http.ResponseWriter to capture the status code
// written by the handler so the metrics middleware can record it.
type responseWriter struct {
	http.ResponseWriter
	statusCode int
	written    bool
}

func newResponseWriter(w http.ResponseWriter) *responseWriter {
	return &responseWriter{ResponseWriter: w, statusCode: http.StatusOK}
}

func (rw *responseWriter) WriteHeader(code int) {
	if !rw.written {
		rw.statusCode = code
		rw.written = true
	}
	rw.ResponseWriter.WriteHeader(code)
}

func (rw *responseWriter) Write(b []byte) (int, error) {
	if !rw.written {
		rw.written = true
	}
	return rw.ResponseWriter.Write(b)
}

// Flush forwards to the underlying ResponseWriter if it implements
// http.Flusher. Required for streaming/proxy responses so that data is
// not buffered indefinitely in the middleware wrapper.
func (rw *responseWriter) Flush() {
	if f, ok := rw.ResponseWriter.(http.Flusher); ok {
		f.Flush()
	}
}

// Hijack forwards to the underlying ResponseWriter so WebSocket upgrades
// (e.g. noVNC's /websockify through /api/svc) survive this wrapper. Struct
// embedding only exposes http.ResponseWriter's interface methods, so without
// this passthrough httputil.ReverseProxy fails every 101 Switching Protocols
// with a 502 ("can't switch protocols using non-Hijacker ResponseWriter").
func (rw *responseWriter) Hijack() (net.Conn, *bufio.ReadWriter, error) {
	hj, ok := rw.ResponseWriter.(http.Hijacker)
	if !ok {
		return nil, nil, fmt.Errorf("underlying ResponseWriter does not implement http.Hijacker")
	}
	if !rw.written {
		rw.statusCode = http.StatusSwitchingProtocols
		rw.written = true
	}
	return hj.Hijack()
}

// Middleware returns an http.Handler that records HTTP SLI metrics for
// each request. It must be placed as the outermost handler wrapper so
// that it captures the final status code from all inner layers (auth,
// OPA, business logic).
func Middleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		rw := newResponseWriter(w)
		ActiveRequests.Inc()
		defer ActiveRequests.Dec()

		next.ServeHTTP(rw, r)

		duration := time.Since(start).Seconds()
		statusStr := strconv.Itoa(rw.statusCode)
		path := normalizePath(r.URL.Path)

		HTTPRequestsTotal.WithLabelValues(r.Method, path, statusStr).Inc()
		HTTPRequestDuration.WithLabelValues(r.Method, path, statusStr).Observe(duration)
	})
}

// normalizePath collapses high-cardinality dynamic path segments so
// Prometheus cardinality stays bounded.
//
// Examples:
//
//	/api/keys/3f2a...  → /api/keys/:id
//	/api/gateway/mypool/reset → /api/gateway/:name/:path
//	/api/orch/ns/svc/api/vms → /api/orch/:namespace/:service/:path
//	/api/k8s/api/v1/pods     → /api/k8s/:path
func normalizePath(p string) string {
	switch {
	case p == "/healthz":
		return "/healthz"
	case p == "/api/keys":
		return "/api/keys"
	case len(p) > 10 && p[:10] == "/api/keys/":
		return "/api/keys/:id"
	case len(p) > 13 && p[:13] == "/api/gateway/":
		return "/api/gateway/:name/:path"
	case len(p) > 9 && p[:9] == "/api/svc/":
		return "/api/svc/:namespace/:service/:path"
	case len(p) > 10 && p[:10] == "/api/orch/":
		return "/api/orch/:namespace/:service/:path"
	case len(p) > 9 && p[:9] == "/api/k8s/":
		return "/api/k8s/:path"
	default:
		return p
	}
}

// RecordKeycloakRequest records a Keycloak admin API operation result.
// operation: CreateKey, ListKeys, DeleteKey, AdminLogin
// success: true on 2xx / no error, false otherwise
func RecordKeycloakRequest(operation string, duration time.Duration, err error) {
	status := "success"
	if err != nil {
		status = "error"
	}
	KeycloakRequestsTotal.WithLabelValues(operation, status).Inc()
	KeycloakRequestDuration.WithLabelValues(operation, status).Observe(duration.Seconds())
}

// RecordUpstreamProxy records a reverse-proxy request to an upstream target.
// target: "gateway", "orch", or "k8s"
// statusCode: HTTP status code returned from upstream (0 if dial failed)
func RecordUpstreamProxy(target string, statusCode int, duration time.Duration) {
	statusStr := strconv.Itoa(statusCode)
	UpstreamProxyRequestsTotal.WithLabelValues(target, statusStr).Inc()
	UpstreamProxyDuration.WithLabelValues(target, statusStr).Observe(duration.Seconds())
}

// StartMetricsServer starts a lightweight HTTP server that exposes
// /metrics in Prometheus exposition format on addr (e.g. ":9090").
// It blocks until the server exits; run it in a goroutine.
func StartMetricsServer(addr string) error {
	mux := http.NewServeMux()
	mux.Handle("/metrics", promhttp.Handler())
	mux.HandleFunc("/healthz", func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("ok"))
	})
	srv := &http.Server{Addr: addr, Handler: mux, ReadHeaderTimeout: 5 * time.Second}
	return srv.ListenAndServe()
}
