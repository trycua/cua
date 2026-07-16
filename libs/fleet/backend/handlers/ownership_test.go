package handlers

import (
	"net/http"
	"net/http/httptest"
	"testing"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/config"
)

func testHandlers() Handlers {
	return Handlers{
		GatewayCfg: config.GatewayConfiguration{
			Scheme:        "http",
			Port:          "80",
			ClusterDomain: "svc.cluster.local",
		},
	}
}

// svcRequest builds a /api/svc request with path values + user stamped.
func svcRequest(u *auth.User, ns, service string) *http.Request {
	r := httptest.NewRequest(http.MethodGet, "/api/svc/"+ns+"/"+service+"/", nil)
	r.SetPathValue("namespace", ns)
	r.SetPathValue("service", service)
	r.SetPathValue("path", "")
	if u != nil {
		r = withUser(r, u)
	}
	return r
}

const probePath = "/apis/rbac.authorization.k8s.io/v1/namespaces/other-ns/rolebindings"

func TestSvc_PerKey_NamespaceMismatch_Forbidden(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := testHandlers()
	w := httptest.NewRecorder()
	h.Svc(w, svcRequest(&auth.User{ID: "svc-acct", AZP: "key-mypool", Namespace: "mypool"}, "other-ns", "novnc"))

	if w.Code != http.StatusForbidden {
		t.Fatalf("status = %d, want 403; body = %s", w.Code, w.Body.String())
	}
	if len(fk.requests) != 0 {
		t.Fatalf("expected no K8s probe for per-key client, got %d request(s)", len(fk.requests))
	}
}

// The "allowed" paths are exercised via requireNamespaceAccess directly:
// going through h.Svc would make the reverse proxy dial the (nonexistent)
// *.svc.cluster.local target and stall the test on DNS timeouts.

func TestRequireNamespaceAccess_PerKey_NamespaceMatch_NoProbe(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := testHandlers()
	w := httptest.NewRecorder()
	r := svcRequest(&auth.User{ID: "svc-acct", AZP: "key-mypool", Namespace: "mypool"}, "mypool", "novnc")
	if !h.requireNamespaceAccess(w, r, currentUser(r), "mypool") {
		t.Fatalf("authorized per-key request rejected; status = %d, body = %s", w.Code, w.Body.String())
	}
	if len(fk.requests) != 0 {
		t.Fatalf("expected no K8s probe for per-key client, got %d request(s)", len(fk.requests))
	}
}

func TestRequireNamespaceAccess_SPAUser_OwnedNamespace_ProbeAllows(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := testHandlers()
	w := httptest.NewRecorder()
	r := svcRequest(&auth.User{ID: "test-uuid", AZP: "cyclops-cs-spa"}, "other-ns", "novnc")
	if !h.requireNamespaceAccess(w, r, currentUser(r), "other-ns") {
		t.Fatalf("owner rejected; status = %d, body = %s", w.Code, w.Body.String())
	}
	if len(fk.requests) != 1 {
		t.Fatalf("expected exactly one probe, got %d", len(fk.requests))
	}
	probe := fk.requests[0]
	if probe.method != http.MethodGet || probe.path != probePath {
		t.Fatalf("probe = %s %q, want GET %s", probe.method, probe.path, probePath)
	}
	if got := probe.headers.Get("Impersonate-User"); got != "oidc:test-uuid" {
		t.Fatalf("Impersonate-User = %q, want oidc:test-uuid", got)
	}
	if got := probe.headers.Get("Impersonate-Group"); got != "oidc:cyclops-cs-tenants" {
		t.Fatalf("Impersonate-Group = %q, want oidc:cyclops-cs-tenants", got)
	}
}

func TestSvc_SPAUser_NonOwnedNamespace_Forbidden(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusForbidden, `{"kind":"Status","status":"Failure","reason":"Forbidden"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := testHandlers()
	w := httptest.NewRecorder()
	h.Svc(w, svcRequest(&auth.User{ID: "intruder-uuid", AZP: "cyclops-cs-spa"}, "other-ns", "novnc"))

	if w.Code != http.StatusForbidden {
		t.Fatalf("status = %d, want 403; body = %s", w.Code, w.Body.String())
	}
}

func TestSvc_OAuth2ProxyUser_NonOwnedNamespace_Forbidden(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusForbidden, `{"kind":"Status","status":"Failure","reason":"Forbidden"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := testHandlers()
	w := httptest.NewRecorder()
	h.Svc(w, svcRequest(&auth.User{ID: "intruder-uuid", AZP: "oauth2-proxy", Email: "x@y.z"}, "other-ns", "novnc"))

	if w.Code != http.StatusForbidden {
		t.Fatalf("status = %d, want 403; body = %s", w.Code, w.Body.String())
	}
}

func TestSvc_K8sUnavailable_FailsClosed(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusInternalServerError, `oops`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := testHandlers()
	for i := 0; i < 2; i++ {
		w := httptest.NewRecorder()
		h.Svc(w, svcRequest(&auth.User{ID: "test-uuid", AZP: "cyclops-cs-spa"}, "other-ns", "novnc"))
		if w.Code != http.StatusBadGateway {
			t.Fatalf("status = %d, want 502; body = %s", w.Code, w.Body.String())
		}
	}
	// Indeterminate verdicts must not be cached: both requests probe.
	if len(fk.requests) != 2 {
		t.Fatalf("expected 2 probes (errors uncached), got %d", len(fk.requests))
	}
}

func TestOwnership_PositiveVerdictCached(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := testHandlers()
	for i := 0; i < 3; i++ {
		w := httptest.NewRecorder()
		r := svcRequest(&auth.User{ID: "test-uuid", AZP: "cyclops-cs-spa"}, "other-ns", "novnc")
		if !h.requireNamespaceAccess(w, r, currentUser(r), "other-ns") {
			t.Fatalf("request %d rejected with %d", i, w.Code)
		}
	}
	if len(fk.requests) != 1 {
		t.Fatalf("expected 1 probe across 3 requests (cached), got %d", len(fk.requests))
	}
}

func TestOwnership_NegativeVerdictCached(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusForbidden, `{"kind":"Status"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := testHandlers()
	for i := 0; i < 2; i++ {
		w := httptest.NewRecorder()
		h.Svc(w, svcRequest(&auth.User{ID: "intruder-uuid", AZP: "cyclops-cs-spa"}, "other-ns", "novnc"))
		if w.Code != http.StatusForbidden {
			t.Fatalf("request %d: status = %d, want 403", i, w.Code)
		}
	}
	if len(fk.requests) != 1 {
		t.Fatalf("expected 1 probe across 2 requests (negative cached), got %d", len(fk.requests))
	}
}

func TestOwnership_CacheIsPerUser(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := testHandlers()
	for _, sub := range []string{"user-a", "user-b"} {
		w := httptest.NewRecorder()
		r := svcRequest(&auth.User{ID: sub, AZP: "cyclops-cs-spa"}, "other-ns", "novnc")
		h.requireNamespaceAccess(w, r, currentUser(r), "other-ns")
	}
	// A cached verdict for user-a must not leak to user-b.
	if len(fk.requests) != 2 {
		t.Fatalf("expected 2 probes (one per user), got %d", len(fk.requests))
	}
}

func TestSvc_InvalidNamespace_BadRequest(t *testing.T) {
	resetOwnershipCache()
	h := testHandlers()
	w := httptest.NewRecorder()
	h.Svc(w, svcRequest(&auth.User{ID: "test-uuid", AZP: "cyclops-cs-spa"}, "Bad_NS", "novnc"))

	if w.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400; body = %s", w.Code, w.Body.String())
	}
}

func TestOrch_NonOwnedNamespace_Forbidden(t *testing.T) {
	resetOwnershipCache()
	fk := newFakeK8s(http.StatusForbidden, `{"kind":"Status"}`)
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := testHandlers()
	r := httptest.NewRequest(http.MethodGet, "/api/orch/other-ns/catalog/items", nil)
	r.SetPathValue("namespace", "other-ns")
	r.SetPathValue("service", "catalog")
	r.SetPathValue("path", "items")
	r = withUser(r, &auth.User{ID: "intruder-uuid", AZP: "cyclops-cs-spa"})
	w := httptest.NewRecorder()
	h.Orch(w, r)

	if w.Code != http.StatusForbidden {
		t.Fatalf("status = %d, want 403; body = %s", w.Code, w.Body.String())
	}
}
