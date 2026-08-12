package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"cyclops-cs-backend/handlers"
)

// RouteContext derives a route's parameter names from its pattern rather than
// from a list registered alongside it. These tests are what stands between that
// derivation and a silent lockout: a parser that produced no names — or that
// kept the "..." of a trailing wildcard in the key — leaves input.params.X
// undefined, the rule body fails, `default allow = false` answers, and every
// route whose policy reads a parameter returns 403 to everyone. Nothing else in
// the suite notices, because failing closed is what a deny test expects too.
//
// So both routes below are driven in a pair. The deny half fails closed either
// way and proves nothing on its own; the allow half is the half that breaks.
//
// The verdicts are the ones auth/testdata/route-authorization-table.txt records
// for the same route, params and principal — a spa token here is the table's
// "spa" row.

// isPolicyDenial reports whether a response is the authorization stage's own
// 403 rather than a handler's. The two are told apart by message: the policy
// stage writes the surface's denied message, and every handler past it writes
// its own vocabulary. Status alone cannot separate them — the handlers deny
// with 403 as well.
func isPolicyDenial(t *testing.T, response *httptest.ResponseRecorder, deniedMessage string) bool {
	t.Helper()
	if response.Code != http.StatusForbidden {
		return false
	}
	var body struct {
		Error string `json:"error"`
	}
	if err := json.Unmarshal(response.Body.Bytes(), &body); err != nil {
		t.Fatalf("decode error body %q: %v", response.Body.String(), err)
	}
	return body.Error == deniedMessage
}

// TestK8sRouteVerdictFollowsThePathParam pins the {path...} wildcard: the two
// requests differ only in input.params.path, which is what authz_k8s.rego's
// infra-path check reads. Both are table rows —
//
//	/api/k8s/{path...} | namespaced-pods | GET | spa = allow
//	/api/k8s/{path...} | cluster-nodes   | GET | spa = deny
//
// — and the allow row is unreachable unless the parameter is keyed "path" with
// no trailing dots.
func TestK8sRouteVerdictFollowsThePathParam(t *testing.T) {
	cases := []struct {
		name           string
		path           string
		wantUpstream   bool
		wantStatusCode int
	}{
		{name: "namespaced pods are allowed", path: "/api/k8s/api/v1/namespaces/ns-a/pods", wantUpstream: true, wantStatusCode: http.StatusOK},
		{name: "cluster-wide nodes are denied", path: "/api/k8s/api/v1/nodes", wantUpstream: false, wantStatusCode: http.StatusForbidden},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			upstreamCalled := false
			upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
				upstreamCalled = true
				w.WriteHeader(http.StatusOK)
			}))
			defer upstream.Close()
			t.Setenv("KUBECTL_PROXY_ADDR", upstream.URL)

			router := setupRouter(handlers.Handlers{})
			response := httptest.NewRecorder()
			router.ServeHTTP(response, authorizedRequest(t, http.MethodGet, testCase.path, nil))

			if response.Code != testCase.wantStatusCode {
				t.Fatalf("status = %d, want %d; body = %s", response.Code, testCase.wantStatusCode, response.Body.String())
			}
			if upstreamCalled != testCase.wantUpstream {
				t.Fatalf("upstream reached = %t, want %t; body = %s", upstreamCalled, testCase.wantUpstream, response.Body.String())
			}
		})
	}
}

// TestSvcRouteVerdictFollowsTheNamespaceParam pins the two mid-path wildcards
// of /api/svc/{namespace}/{service}, which authz_svc.rego shape-checks as DNS
// labels. Both are table rows —
//
//	/api/svc/{namespace}/{service} | owned-ns   | GET | spa = allow
//	/api/svc/{namespace}/{service} | invalid-ns | GET | spa = deny
//
// The allowed request is not asserted to succeed: past the policy it reaches
// handlers.Svc, whose namespace-ownership probe has no K8s client in a test and
// fails closed. What is asserted is that the 403 it does not get is the policy's
// — which is exactly the verdict the table records.
func TestSvcRouteVerdictFollowsTheNamespaceParam(t *testing.T) {
	// The svc surface takes the default denied message; see auth.PolicyMiddleware.
	const svcDeniedMessage = "forbidden"

	router := setupRouter(handlers.Handlers{})

	allowed := httptest.NewRecorder()
	router.ServeHTTP(allowed, authorizedRequest(t, http.MethodGet, "/api/svc/ns-a/svc-a", nil))
	if isPolicyDenial(t, allowed, svcDeniedMessage) {
		t.Fatalf("a valid namespace and service were denied by the policy; body = %s", allowed.Body.String())
	}

	denied := httptest.NewRecorder()
	router.ServeHTTP(denied, authorizedRequest(t, http.MethodGet, "/api/svc/Not_A_Label/svc-a", nil))
	if !isPolicyDenial(t, denied, svcDeniedMessage) {
		t.Fatalf("a namespace that is not a DNS label was not denied by the policy; status = %d, body = %s",
			denied.Code, denied.Body.String())
	}
}
