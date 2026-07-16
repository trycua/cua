package middlewares

import (
	"bytes"
	"context"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"go.opentelemetry.io/otel"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/trace"
)

func TestLogMiddleware_EmitsTraceAndSpanIDs(t *testing.T) {
	t.Setenv("OTEL_SERVICE_NAME", "cyclops-cs-backend")
	t.Setenv("OTEL_SERVICE_NAMESPACE", "cyclops-cs")
	t.Setenv("OTEL_ENVIRONMENT", "production")

	tp := sdktrace.NewTracerProvider()
	ctx, span := tp.Tracer("test").Start(context.Background(), "request")
	defer span.End()

	var buf bytes.Buffer
	logger := slog.New(slog.NewJSONHandler(&buf, nil))
	req := httptest.NewRequest(http.MethodGet, "/healthz", nil).WithContext(ctx)
	req = req.WithContext(context.WithValue(req.Context(), ContextKey("route"), "/healthz"))
	rr := httptest.NewRecorder()

	LogMiddlewareWithLogger(logger)(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	})).ServeHTTP(rr, req)

	logLine := buf.String()
	if !strings.Contains(logLine, "trace_id") {
		t.Fatalf("log line missing trace_id: %s", logLine)
	}
	if !strings.Contains(logLine, "span_id") {
		t.Fatalf("log line missing span_id: %s", logLine)
	}
	if !strings.Contains(logLine, "\"route\":\"/healthz\"") {
		t.Fatalf("log line missing route: %s", logLine)
	}
	if !strings.Contains(logLine, "\"service_name\":\"cyclops-cs-backend\"") {
		t.Fatalf("log line missing service_name: %s", logLine)
	}
}

func TestTraceMiddleware_StartsSpanAndStoresRoute(t *testing.T) {
	tp := sdktrace.NewTracerProvider()
	otel.SetTracerProvider(tp)
	t.Cleanup(func() {
		_ = tp.Shutdown(context.Background())
	})

	var (
		gotRoute string
		hasSpan  bool
	)

	req := httptest.NewRequest(http.MethodGet, "/api/keys", nil)
	rr := httptest.NewRecorder()

	handler := TraceMiddleware("/api/keys", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotRoute, _ = r.Context().Value(ContextKey("route")).(string)
		hasSpan = trace.SpanContextFromContext(r.Context()).IsValid()
		w.WriteHeader(http.StatusOK)
	}))

	handler.ServeHTTP(rr, req)

	if gotRoute != "/api/keys" {
		t.Fatalf("route = %q, want %q", gotRoute, "/api/keys")
	}
	if !hasSpan {
		t.Fatal("expected valid span context")
	}
}
