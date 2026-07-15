package handlers

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"cyclops-cs-backend/auth"
	"go.opentelemetry.io/otel"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/sdk/trace/tracetest"
)

// withUser stamps a *auth.User on the request context the same way
// TokenAuthMiddleware does in production.
func withUser(r *http.Request, u *auth.User) *http.Request {
	return r.WithContext(context.WithValue(r.Context(), auth.UserKey, u))
}

// fakeK8sProxy returns an httptest.Server that records every request it
// receives (most-recent also exposed via the flat fields) and replies with the
// canned status/body. The caller can inspect recorded headers and path after
// the handler runs.
type recordedReq struct {
	method   string
	path     string
	rawQuery string
	headers  http.Header
	body     []byte
}

type fakeK8s struct {
	server *httptest.Server

	// recorded from the last request
	method   string
	path     string
	rawQuery string
	headers  http.Header
	body     []byte

	// every request, in order
	requests []recordedReq
}

func newFakeK8s(status int, respBody string) *fakeK8s {
	fk := &fakeK8s{}
	fk.server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		fk.method = r.Method
		fk.path = r.URL.Path
		fk.rawQuery = r.URL.RawQuery
		fk.headers = r.Header.Clone()
		fk.body, _ = io.ReadAll(r.Body)
		fk.requests = append(fk.requests, recordedReq{
			method:   fk.method,
			path:     fk.path,
			rawQuery: fk.rawQuery,
			headers:  fk.headers,
			body:     fk.body,
		})
		w.WriteHeader(status)
		_, _ = w.Write([]byte(respBody))
	}))
	return fk
}

// nsListResponse builds a minimal K8s NamespaceList JSON.
func nsListResponse(names ...string) string {
	type nsMeta struct {
		Name              string            `json:"name"`
		Labels            map[string]string `json:"labels"`
		CreationTimestamp string            `json:"creationTimestamp"`
	}
	type nsStatus struct {
		Phase string `json:"phase"`
	}
	type nsItem struct {
		Metadata nsMeta   `json:"metadata"`
		Status   nsStatus `json:"status"`
	}
	items := make([]nsItem, 0, len(names))
	for _, n := range names {
		items = append(items, nsItem{
			Metadata: nsMeta{Name: n, Labels: map[string]string{}, CreationTimestamp: "2026-05-28T00:00:00Z"},
			Status:   nsStatus{Phase: "Active"},
		})
	}
	b, _ := json.Marshal(map[string]any{"items": items})
	return string(b)
}

// nsCreatedResponse builds a minimal K8s Namespace JSON (single object).
func nsCreatedResponse(name string) string {
	b, _ := json.Marshal(map[string]any{
		"metadata": map[string]any{
			"name":              name,
			"labels":            map[string]string{},
			"creationTimestamp": "2026-05-28T00:00:00Z",
		},
		"status": map[string]any{"phase": "Active"},
	})
	return string(b)
}

func TestCreateNamespace_ValidName(t *testing.T) {
	fk := newFakeK8s(http.StatusCreated, nsCreatedResponse("my-workspace"))
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := Handlers{}
	body := `{"name":"my-workspace"}`
	r := httptest.NewRequest(http.MethodPost, "/api/namespaces", strings.NewReader(body))
	r = withUser(r, &auth.User{ID: "test-uuid"})
	w := httptest.NewRecorder()

	h.CreateNamespace(w, r)

	if w.Code != http.StatusCreated {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusCreated, w.Body.String())
	}

	// The handler issues the create POST followed by the adoption-wait GET
	// probe; inspect the POST explicitly.
	if len(fk.requests) < 2 {
		t.Fatalf("expected create POST + adoption probe GET, got %d request(s)", len(fk.requests))
	}
	post := fk.requests[0]

	// Verify impersonation headers sent to k8s proxy.
	if got := post.headers.Get("Impersonate-User"); got != "oidc:test-uuid" {
		t.Fatalf("Impersonate-User = %q, want %q", got, "oidc:test-uuid")
	}
	if got := post.headers.Get("Impersonate-Group"); got != "oidc:cyclops-cs-tenants" {
		t.Fatalf("Impersonate-Group = %q, want %q", got, "oidc:cyclops-cs-tenants")
	}

	// Verify path.
	if post.method != http.MethodPost || post.path != "/api/v1/namespaces" {
		t.Fatalf("upstream req = %s %q, want POST /api/v1/namespaces", post.method, post.path)
	}

	// Verify the K8s body contains the namespace name.
	var nsObj map[string]any
	if err := json.Unmarshal(post.body, &nsObj); err != nil {
		t.Fatalf("upstream body not JSON: %v", err)
	}
	meta, _ := nsObj["metadata"].(map[string]any)
	if meta["name"] != "my-workspace" {
		t.Fatalf("upstream ns name = %v, want my-workspace", meta["name"])
	}

	// Verify the adoption probe: an impersonated RoleBinding LIST in the new
	// namespace (only authorized once Capsule's owner RoleBindings exist —
	// unlike a namespace GET, which system:authenticated can always do).
	probe := fk.requests[len(fk.requests)-1]
	wantProbe := "/apis/rbac.authorization.k8s.io/v1/namespaces/my-workspace/rolebindings"
	if probe.method != http.MethodGet || probe.path != wantProbe {
		t.Fatalf("probe req = %s %q, want GET %s", probe.method, probe.path, wantProbe)
	}
	if got := probe.headers.Get("Impersonate-User"); got != "oidc:test-uuid" {
		t.Fatalf("probe Impersonate-User = %q, want %q", got, "oidc:test-uuid")
	}

	// Verify response body.
	var resp NamespaceResponse
	if err := json.NewDecoder(w.Body).Decode(&resp); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	if resp.Name != "my-workspace" {
		t.Fatalf("response name = %q, want my-workspace", resp.Name)
	}
}

func TestCreateNamespace_InvalidName(t *testing.T) {
	h := Handlers{}

	cases := []struct {
		name    string
		payload string
	}{
		{"uppercase", `{"name":"MyWorkspace"}`},
		{"special chars", `{"name":"my_workspace!"}`},
		{"starts with dash", `{"name":"-bad"}`},
		{"too long", `{"name":"` + strings.Repeat("a", 64) + `"}`},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			r := httptest.NewRequest(http.MethodPost, "/api/namespaces", strings.NewReader(tc.payload))
			r = withUser(r, &auth.User{ID: "test-uuid"})
			w := httptest.NewRecorder()

			h.CreateNamespace(w, r)

			if w.Code != http.StatusBadRequest {
				t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusBadRequest, w.Body.String())
			}
		})
	}
}

func TestCreateNamespace_MissingBody(t *testing.T) {
	h := Handlers{}

	t.Run("empty body", func(t *testing.T) {
		r := httptest.NewRequest(http.MethodPost, "/api/namespaces", strings.NewReader(""))
		r = withUser(r, &auth.User{ID: "test-uuid"})
		w := httptest.NewRecorder()

		h.CreateNamespace(w, r)

		if w.Code != http.StatusBadRequest {
			t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusBadRequest, w.Body.String())
		}
	})

	t.Run("empty name", func(t *testing.T) {
		r := httptest.NewRequest(http.MethodPost, "/api/namespaces", strings.NewReader(`{"name":""}`))
		r = withUser(r, &auth.User{ID: "test-uuid"})
		w := httptest.NewRecorder()

		h.CreateNamespace(w, r)

		if w.Code != http.StatusBadRequest {
			t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusBadRequest, w.Body.String())
		}
	})

	t.Run("no JSON", func(t *testing.T) {
		r := httptest.NewRequest(http.MethodPost, "/api/namespaces", bytes.NewReader([]byte("not-json")))
		r = withUser(r, &auth.User{ID: "test-uuid"})
		w := httptest.NewRecorder()

		h.CreateNamespace(w, r)

		if w.Code != http.StatusBadRequest {
			t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusBadRequest, w.Body.String())
		}
	})
}

func TestCreateNamespace_MissingUser(t *testing.T) {
	h := Handlers{}
	body := `{"name":"my-workspace"}`
	r := httptest.NewRequest(http.MethodPost, "/api/namespaces", strings.NewReader(body))
	// No withUser — context has no user.
	w := httptest.NewRecorder()

	h.CreateNamespace(w, r)

	if w.Code != http.StatusUnauthorized {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusUnauthorized, w.Body.String())
	}
}

func TestCreateNamespace_CreatesTracingSpan(t *testing.T) {
	fk := newFakeK8s(http.StatusCreated, nsCreatedResponse("my-workspace"))
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	recorder := tracetest.NewSpanRecorder()
	tp := sdktrace.NewTracerProvider(sdktrace.WithSpanProcessor(recorder))
	otel.SetTracerProvider(tp)

	ctx, root := tp.Tracer("test").Start(context.Background(), "request")

	h := Handlers{}
	r := httptest.NewRequest(http.MethodPost, "/api/namespaces", strings.NewReader(`{"name":"my-workspace"}`)).WithContext(ctx)
	r = withUser(r, &auth.User{ID: "test-uuid"})
	w := httptest.NewRecorder()

	h.CreateNamespace(w, r)
	root.End()

	var found bool
	for _, span := range recorder.Ended() {
		if span.Name() != "namespaces.create" {
			continue
		}
		found = true
		if got, want := span.Parent().SpanID(), root.SpanContext().SpanID(); got != want {
			t.Fatalf("parent span id = %s, want %s", got, want)
		}
	}
	if !found {
		t.Fatal("expected namespaces.create span")
	}
}

func TestDeleteNamespace_Success(t *testing.T) {
	fk := newFakeK8s(http.StatusOK, `{"kind":"Status","status":"Success"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := Handlers{}
	r := httptest.NewRequest(http.MethodDelete, "/api/namespaces/my-workspace", nil)
	r.SetPathValue("name", "my-workspace")
	r = withUser(r, &auth.User{ID: "test-uuid"})
	w := httptest.NewRecorder()

	h.DeleteNamespace(w, r)

	if w.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusNoContent, w.Body.String())
	}

	// Verify impersonation headers.
	if got := fk.headers.Get("Impersonate-User"); got != "oidc:test-uuid" {
		t.Fatalf("Impersonate-User = %q, want %q", got, "oidc:test-uuid")
	}
	if got := fk.headers.Get("Impersonate-Group"); got != "oidc:cyclops-cs-tenants" {
		t.Fatalf("Impersonate-Group = %q, want %q", got, "oidc:cyclops-cs-tenants")
	}

	// Verify DELETE was sent to the right path.
	if fk.method != http.MethodDelete {
		t.Fatalf("upstream method = %q, want DELETE", fk.method)
	}
	if fk.path != "/api/v1/namespaces/my-workspace" {
		t.Fatalf("upstream path = %q, want /api/v1/namespaces/my-workspace", fk.path)
	}
}

func TestDeleteNamespace_MissingUser(t *testing.T) {
	h := Handlers{}
	r := httptest.NewRequest(http.MethodDelete, "/api/namespaces/my-workspace", nil)
	r.SetPathValue("name", "my-workspace")
	// No user context.
	w := httptest.NewRecorder()

	h.DeleteNamespace(w, r)

	if w.Code != http.StatusUnauthorized {
		t.Fatalf("status = %d, want %d", w.Code, http.StatusUnauthorized)
	}
}

func TestDeleteNamespace_NotFound(t *testing.T) {
	fk := newFakeK8s(http.StatusNotFound, `{"kind":"Status","status":"Failure","reason":"NotFound"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := Handlers{}
	r := httptest.NewRequest(http.MethodDelete, "/api/namespaces/no-such-ns", nil)
	r.SetPathValue("name", "no-such-ns")
	r = withUser(r, &auth.User{ID: "test-uuid"})
	w := httptest.NewRecorder()

	h.DeleteNamespace(w, r)

	if w.Code != http.StatusNotFound {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusNotFound, w.Body.String())
	}
}

func TestListNamespaces_Success(t *testing.T) {
	fk := newFakeK8s(http.StatusOK, nsListResponse("ns-alpha", "ns-beta"))
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := Handlers{}
	r := httptest.NewRequest(http.MethodGet, "/api/namespaces", nil)
	r = withUser(r, &auth.User{ID: "test-uuid"})
	w := httptest.NewRecorder()

	h.ListNamespaces(w, r)

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusOK, w.Body.String())
	}

	// Verify impersonation headers.
	if got := fk.headers.Get("Impersonate-User"); got != "oidc:test-uuid" {
		t.Fatalf("Impersonate-User = %q, want %q", got, "oidc:test-uuid")
	}
	if got := fk.headers.Get("Impersonate-Group"); got != "oidc:cyclops-cs-tenants" {
		t.Fatalf("Impersonate-Group = %q, want %q", got, "oidc:cyclops-cs-tenants")
	}

	// Verify the list is scoped to the caller's Tenant by label selector —
	// this is the fail-closed boundary that does not depend on Capsule Proxy.
	if fk.path != "/api/v1/namespaces" {
		t.Fatalf("upstream path = %q, want /api/v1/namespaces", fk.path)
	}
	q, err := url.ParseQuery(fk.rawQuery)
	if err != nil {
		t.Fatalf("parse upstream query %q: %v", fk.rawQuery, err)
	}
	if got, want := q.Get("labelSelector"), "capsule.clastix.io/tenant=user-test-uuid"; got != want {
		t.Fatalf("labelSelector = %q, want %q", got, want)
	}

	// Parse the response body.
	var nsList []NamespaceResponse
	if err := json.NewDecoder(w.Body).Decode(&nsList); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	if len(nsList) != 2 {
		t.Fatalf("namespace count = %d, want 2", len(nsList))
	}
	if nsList[0].Name != "ns-alpha" {
		t.Fatalf("first namespace = %q, want ns-alpha", nsList[0].Name)
	}
	if nsList[1].Name != "ns-beta" {
		t.Fatalf("second namespace = %q, want ns-beta", nsList[1].Name)
	}
}

func TestListNamespaces_MissingUser(t *testing.T) {
	h := Handlers{}
	r := httptest.NewRequest(http.MethodGet, "/api/namespaces", nil)
	// No user context.
	w := httptest.NewRecorder()

	h.ListNamespaces(w, r)

	if w.Code != http.StatusUnauthorized {
		t.Fatalf("status = %d, want %d", w.Code, http.StatusUnauthorized)
	}
}
