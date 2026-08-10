package metrics

import (
	"testing"
)

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
