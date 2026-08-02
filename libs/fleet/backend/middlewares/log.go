package middlewares

import (
	"bufio"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"os"
	"time"

	"go.opentelemetry.io/otel/trace"
)

type logResponseWriter struct {
	http.ResponseWriter
	statusCode int
}

func (w *logResponseWriter) WriteHeader(code int) {
	w.statusCode = code
	w.ResponseWriter.WriteHeader(code)
}

// Hijack forwards to the underlying ResponseWriter so WebSocket upgrades
// (e.g. noVNC's /websockify through /api/svc) survive this wrapper. Struct
// embedding only exposes http.ResponseWriter's interface methods, so without
// this passthrough httputil.ReverseProxy fails every 101 Switching Protocols
// with a 502 ("can't switch protocols using non-Hijacker ResponseWriter").
func (w *logResponseWriter) Hijack() (net.Conn, *bufio.ReadWriter, error) {
	hj, ok := w.ResponseWriter.(http.Hijacker)
	if !ok {
		return nil, nil, fmt.Errorf("underlying ResponseWriter does not implement http.Hijacker")
	}
	w.statusCode = http.StatusSwitchingProtocols
	return hj.Hijack()
}

// Flush forwards to the underlying ResponseWriter so streaming responses
// (SSE, chunked proxying) are not buffered by the wrapper.
func (w *logResponseWriter) Flush() {
	if f, ok := w.ResponseWriter.(http.Flusher); ok {
		f.Flush()
	}
}

func LogMiddleware(next http.Handler) http.Handler {
	return LogMiddlewareWithLogger(slog.Default())(next)
}

func LogMiddlewareWithLogger(logger *slog.Logger) func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			start := time.Now()
			lw := &logResponseWriter{ResponseWriter: w, statusCode: http.StatusOK}
			next.ServeHTTP(lw, r)

			spanCtx := trace.SpanContextFromContext(r.Context())
			route, _ := r.Context().Value(ContextKey("route")).(string)
			if route == "" {
				route = r.URL.Path
			}

			attrs := []any{
				"service_name", envOrDefault("OTEL_SERVICE_NAME", "cyclops-cs-backend"),
				"service_namespace", envOrDefault("OTEL_SERVICE_NAMESPACE", "cyclops-cs"),
				"deployment_environment", envOrDefault("OTEL_ENVIRONMENT", "production"),
				"method", r.Method,
				"url", r.URL.Path,
				"route", route,
				"duration", time.Since(start),
				"status", lw.statusCode,
			}
			if spanCtx.IsValid() {
				attrs = append(attrs,
					"trace_id", spanCtx.TraceID().String(),
					"span_id", spanCtx.SpanID().String(),
				)
			}

			logger.Info("WebRequest", attrs...)
		})
	}
}

func envOrDefault(key, fallback string) string {
	if value := os.Getenv(key); value != "" {
		return value
	}
	return fallback
}
