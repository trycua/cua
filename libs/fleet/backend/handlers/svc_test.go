package handlers

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"

	"cyclops-cs-backend/config"
)

func TestRewriteLocation(t *testing.T) {
	tests := []struct {
		name     string
		location string
		basePath string
		want     string
	}{
		{"absolute /llms.txt", "/llms.txt", "/api/svc/myns/mysvc", "/api/svc/myns/mysvc/llms.txt"},
		{"absolute /foo/bar", "/foo/bar", "/api/svc/pool-test/test-abc-mcp", "/api/svc/pool-test/test-abc-mcp/foo/bar"},
		{"root /", "/", "/api/svc/ns/svc", "/api/svc/ns/svc/"},
		{"relative path", "other.html", "/api/svc/ns/svc", "/api/svc/ns/svc/other.html"},
		{"external URL unchanged", "https://example.com/foo", "/api/svc/ns/svc", "https://example.com/foo"},
		{"empty unchanged", "", "/api/svc/ns/svc", ""},
		{"absolute with query", "/search?q=test", "/api/svc/ns/svc", "/api/svc/ns/svc/search?q=test"},
		{"absolute with fragment", "/page#section", "/api/svc/ns/svc", "/api/svc/ns/svc/page#section"},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := rewriteLocation(tc.location, tc.basePath)
			if got != tc.want {
				t.Errorf("rewriteLocation(%q, %q) = %q, want %q", tc.location, tc.basePath, got, tc.want)
			}
		})
	}
}

func TestSvc_Unauthenticated(t *testing.T) {
	h := Handlers{
		GatewayCfg: config.GatewayConfiguration{
			Scheme:        "http",
			Port:          "80",
			ClusterDomain: "svc.cluster.local",
		},
	}

	w := httptest.NewRecorder()
	r := httptest.NewRequest("GET", "/api/svc/ns/svc/path", nil)
	r.SetPathValue("namespace", "ns")
	r.SetPathValue("service", "svc")
	r.SetPathValue("path", "path")

	h.Svc(w, r)

	if w.Code != http.StatusUnauthorized {
		t.Errorf("expected 401, got %d", w.Code)
	}
}

func TestSvcProxyErrorStatus(t *testing.T) {
	tests := []struct {
		name string
		err  error
		want int
	}{
		{name: "client canceled", err: context.Canceled, want: 499},
		{name: "wrapped client canceled", err: errors.Join(errors.New("proxy"), context.Canceled), want: 499},
		{name: "other proxy error", err: errors.New("dial tcp: no route to host"), want: http.StatusBadGateway},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if got := svcProxyErrorStatus(tc.err); got != tc.want {
				t.Fatalf("svcProxyErrorStatus(%v) = %d, want %d", tc.err, got, tc.want)
			}
		})
	}
}
