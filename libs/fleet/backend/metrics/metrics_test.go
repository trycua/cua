package metrics

import (
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/prometheus/client_golang/prometheus"
)

func TestMiddlewareRecordsRequestUserOnLatencyHistogram(t *testing.T) {
	handler := Middleware(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		SetRequestUser(r.Context(), "user-123")
		w.WriteHeader(http.StatusNoContent)
	}))
	handler.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest(http.MethodGet, "/api/keys", nil))

	if count := histogramCount(t, map[string]string{
		"method":      "GET",
		"path":        "/api/keys",
		"status_code": "204",
		"user":        "user-123",
	}); count != 1 {
		t.Fatalf("request histogram count = %d, want 1", count)
	}
}

func TestMiddlewareRecordsUnknownUserByDefault(t *testing.T) {
	handler := Middleware(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	handler.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest(http.MethodGet, "/healthz", nil))

	if count := histogramCount(t, map[string]string{
		"method":      "GET",
		"path":        "/healthz",
		"status_code": "200",
		"user":        "unknown",
	}); count != 1 {
		t.Fatalf("request histogram count = %d, want 1", count)
	}
}

func histogramCount(t *testing.T, labels map[string]string) uint64 {
	t.Helper()
	families, err := prometheus.DefaultGatherer.Gather()
	if err != nil {
		t.Fatal(err)
	}
	for _, family := range families {
		if family.GetName() != "cyclops_cs_http_request_duration_seconds" {
			continue
		}
		for _, metric := range family.Metric {
			matched := len(metric.Label) == len(labels)
			for _, pair := range metric.Label {
				matched = matched && labels[pair.GetName()] == pair.GetValue()
			}
			if matched {
				return metric.GetHistogram().GetSampleCount()
			}
		}
	}
	return 0
}

func TestNormalizePath(t *testing.T) {
	cases := []struct {
		in   string
		want string
	}{
		{"/healthz", "/healthz"},
		{"/api/keys", "/api/keys"},
		{"/api/keys/3f2a1b2c3d4e", "/api/keys/:id"},
		{"/api/keys/abc-def-ghi", "/api/keys/:id"},
		{"/api/gateway/mypool", "/api/gateway/:name/:path"},
		{"/api/gateway/mypool/reset", "/api/gateway/:name/:path"},
		{"/api/gateway/mypool/vms/list", "/api/gateway/:name/:path"},
		{"/api/svc/pool-mypool/my-svc/health", "/api/svc/:namespace/:service/:path"},
		{"/api/svc/pool-mypool/my-svc", "/api/svc/:namespace/:service/:path"},
		{"/api/orch/myns/svc/api/vms", "/api/orch/:namespace/:service/:path"},
		{"/api/k8s/api/v1/pods", "/api/k8s/:path"},
		{"/api/k8s/apis/apps/v1/deployments", "/api/k8s/:path"},
		{"/api/swagger/doc.json", "/api/swagger/doc.json"},
		{"/unknown", "/unknown"},
		{"", ""},
	}
	for _, tc := range cases {
		got := normalizePath(tc.in)
		if got != tc.want {
			t.Errorf("normalizePath(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}
