package handlers

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"

	"cyclops-cs-backend/auth"
	"go.opentelemetry.io/otel"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/sdk/trace/tracetest"
)

func TestK8sProxy_ImpersonationHeaders(t *testing.T) {
	var receivedHeaders http.Header
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		receivedHeaders = r.Header.Clone()
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`{"kind":"PodList","items":[]}`))
	}))
	defer upstream.Close()
	t.Setenv("KUBECTL_PROXY_ADDR", upstream.URL)

	h := Handlers{}

	r := httptest.NewRequest(http.MethodGet, "/api/k8s/api/v1/namespaces/foo/pods", nil)
	r.SetPathValue("path", "api/v1/namespaces/foo/pods")
	r = withUser(r, &auth.User{ID: "user-abc-123"})
	w := httptest.NewRecorder()

	h.K8s(w, r)

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusOK, w.Body.String())
	}

	// Verify impersonation headers were added.
	if got := receivedHeaders.Get("Impersonate-User"); got != "oidc:user-abc-123" {
		t.Fatalf("Impersonate-User = %q, want %q", got, "oidc:user-abc-123")
	}
	if got := receivedHeaders.Get("Impersonate-Group"); got != "oidc:cyclops-cs-tenants" {
		t.Fatalf("Impersonate-Group = %q, want %q", got, "oidc:cyclops-cs-tenants")
	}

	// Authorization header must be stripped.
	if got := receivedHeaders.Get("Authorization"); got != "" {
		t.Fatalf("Authorization header should be stripped, got %q", got)
	}

	// Note: Go's HTTP client moves Host into r.Host, so checking the
	// header map is unreliable. The Director sets both req.Host and
	// the "Host" header; the important thing is that the proxy reaches
	// our test server (which accepts any host) and the Director logic
	// is exercised.
}

func TestK8sProxy_NoUser(t *testing.T) {
	var receivedHeaders http.Header
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		receivedHeaders = r.Header.Clone()
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`{"kind":"PodList","items":[]}`))
	}))
	defer upstream.Close()
	t.Setenv("KUBECTL_PROXY_ADDR", upstream.URL)

	h := Handlers{}
	r := httptest.NewRequest(http.MethodGet, "/api/k8s/api/v1/namespaces/foo/pods", nil)
	r.SetPathValue("path", "api/v1/namespaces/foo/pods")
	// No user context — backward compat: no impersonation headers.
	w := httptest.NewRecorder()

	h.K8s(w, r)

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusOK, w.Body.String())
	}

	if got := receivedHeaders.Get("Impersonate-User"); got != "" {
		t.Fatalf("Impersonate-User should be absent, got %q", got)
	}
	if got := receivedHeaders.Get("Impersonate-Group"); got != "" {
		t.Fatalf("Impersonate-Group should be absent, got %q", got)
	}
}

func TestK8sProxy_ForwardsPath(t *testing.T) {
	cases := []struct {
		name       string
		pathValue  string
		wantPath   string
		wantQuery  string
		queryParam string
	}{
		{
			name:      "simple namespaced path",
			pathValue: "api/v1/namespaces/foo/pods",
			wantPath:  "/api/v1/namespaces/foo/pods",
		},
		{
			name:      "cluster-wide path",
			pathValue: "api/v1/nodes",
			wantPath:  "/api/v1/nodes",
		},
		{
			name:      "CRD path",
			pathValue: "apis/cua.ai/v1/osgymworkspacepools",
			wantPath:  "/apis/cua.ai/v1/osgymworkspacepools",
		},
		{
			name:       "path with query params",
			pathValue:  "api/v1/namespaces/foo/events",
			queryParam: "fieldSelector=reason%3DPoolCreated",
			wantPath:   "/api/v1/namespaces/foo/events",
			wantQuery:  "fieldSelector=reason%3DPoolCreated",
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			var receivedPath string
			var receivedQuery string
			upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				receivedPath = r.URL.Path
				receivedQuery = r.URL.RawQuery
				w.WriteHeader(http.StatusOK)
				_, _ = w.Write([]byte(`{}`))
			}))
			defer upstream.Close()
			t.Setenv("KUBECTL_PROXY_ADDR", upstream.URL)

			h := Handlers{}
			target := "/api/k8s/" + tc.pathValue
			if tc.queryParam != "" {
				target += "?" + tc.queryParam
			}
			r := httptest.NewRequest(http.MethodGet, target, nil)
			r.SetPathValue("path", tc.pathValue)
			w := httptest.NewRecorder()

			h.K8s(w, r)

			if w.Code != http.StatusOK {
				t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusOK, w.Body.String())
			}
			if receivedPath != tc.wantPath {
				t.Fatalf("upstream path = %q, want %q", receivedPath, tc.wantPath)
			}
			if tc.wantQuery != "" && receivedQuery != tc.wantQuery {
				t.Fatalf("upstream query = %q, want %q", receivedQuery, tc.wantQuery)
			}
		})
	}
}

func TestK8sProxy_ResponseBody(t *testing.T) {
	// Verify the proxy faithfully forwards the upstream response body.
	wantBody := `{"kind":"PodList","items":[{"metadata":{"name":"pod-1"}}]}`
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(wantBody))
	}))
	defer upstream.Close()
	t.Setenv("KUBECTL_PROXY_ADDR", upstream.URL)

	h := Handlers{}
	r := httptest.NewRequest(http.MethodGet, "/api/k8s/api/v1/namespaces/test/pods", nil)
	r.SetPathValue("path", "api/v1/namespaces/test/pods")
	w := httptest.NewRecorder()

	h.K8s(w, r)

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d", w.Code, http.StatusOK)
	}

	gotBody, _ := io.ReadAll(w.Result().Body)
	var got, want map[string]any
	if err := json.Unmarshal(gotBody, &got); err != nil {
		t.Fatalf("response not JSON: %v", err)
	}
	if err := json.Unmarshal([]byte(wantBody), &want); err != nil {
		t.Fatalf("want not JSON: %v", err)
	}
	if got["kind"] != want["kind"] {
		t.Fatalf("kind = %v, want %v", got["kind"], want["kind"])
	}
}

func TestK8sProxy_CreatesTracingSpan(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`{"kind":"PodList","items":[]}`))
	}))
	defer upstream.Close()
	t.Setenv("KUBECTL_PROXY_ADDR", upstream.URL)

	recorder := tracetest.NewSpanRecorder()
	tp := sdktrace.NewTracerProvider(sdktrace.WithSpanProcessor(recorder))
	otel.SetTracerProvider(tp)

	ctx, root := tp.Tracer("test").Start(context.Background(), "request")

	h := Handlers{}
	r := httptest.NewRequest(http.MethodGet, "/api/k8s/api/v1/namespaces/foo/pods", nil).WithContext(ctx)
	r.SetPathValue("path", "api/v1/namespaces/foo/pods")
	r = withUser(r, &auth.User{ID: "user-abc-123"})
	w := httptest.NewRecorder()

	h.K8s(w, r)
	root.End()

	var found bool
	for _, span := range recorder.Ended() {
		if span.Name() != "k8s.proxy" {
			continue
		}
		found = true
		if got, want := span.Parent().SpanID(), root.SpanContext().SpanID(); got != want {
			t.Fatalf("parent span id = %s, want %s", got, want)
		}
	}
	if !found {
		t.Fatal("expected k8s.proxy span")
	}
}

func TestK8sProxy_RejectsDisallowedPoolImagePullSecret(t *testing.T) {
	auth.LoadOpa()
	upstreamCalled := false
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upstreamCalled = true
		w.WriteHeader(http.StatusCreated)
	}))
	defer upstream.Close()
	t.Setenv("KUBECTL_PROXY_ADDR", upstream.URL)

	body := []byte(`{"spec":{"template":{"containerDiskImage":"evil.example/workspace:latest","imagePullSecret":"ecr-credentials"}}}`)
	r := httptest.NewRequest(http.MethodPost, "/api/k8s/apis/cua.ai/v1/namespaces/foo/osgymworkspacepools", bytes.NewReader(body))
	r.SetPathValue("path", "apis/cua.ai/v1/namespaces/foo/osgymworkspacepools")
	w := httptest.NewRecorder()

	(Handlers{}).K8s(w, r)

	if w.Code != http.StatusForbidden {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusForbidden, w.Body.String())
	}
	if upstreamCalled {
		t.Fatal("disallowed pool request reached kubectl proxy")
	}
}

func TestBodyRequestsMacOS(t *testing.T) {
	cases := []struct {
		name string
		body string
		want bool
	}{
		{"runtime macos", `{"spec":{"template":{"runtime":"macos","containerDiskImage":"img"}}}`, true},
		{"runtime MacOS mixed-case", `{"spec":{"template":{"runtime":"MacOS"}}}`, true},
		{"macos runtimeClass", `{"spec":{"template":{"runtimeClassName":"cua-macos-native"}}}`, true},
		{"macos nodeSelector", `{"spec":{"template":{"nodeSelector":{"cua.ai/macos":"true"}}}}`, true},
		{"kubevirt default", `{"spec":{"template":{"containerDiskImage":"img","cpuCores":4}}}`, false},
		{"no template", `{"spec":{"replicas":1}}`, false},
		{"garbage", `not json`, false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := bodyRequestsMacOS([]byte(c.body)); got != c.want {
				t.Fatalf("bodyRequestsMacOS(%s) = %v, want %v", c.body, got, c.want)
			}
		})
	}
}
