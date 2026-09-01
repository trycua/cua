package handlers

import (
	"context"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"

	"cyclops-cs-backend/auth"
)

type fakeAttributionFactsReader struct {
	claim BoundClaim
	pool  bool
	calls int
}

func (f *fakeAttributionFactsReader) ReadBoundClaimForSandbox(context.Context, string, string) (BoundClaim, error) {
	f.calls++
	return f.claim, nil
}
func (f *fakeAttributionFactsReader) PoolExists(context.Context, string, string) (bool, error) {
	f.calls++
	return f.pool, nil
}

func TestFleetAttributionFactsReaderDerivesClaimFromSandboxOwnerAndExactPool(t *testing.T) {
	var paths []string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		paths = append(paths, r.URL.Path)
		switch r.URL.Path {
		case "/apis/osgym.cua.ai/v1alpha1/namespaces/pool-1/osgymsandboxes/sandbox-1":
			_, _ = w.Write([]byte(`{"metadata":{"ownerReferences":[{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","name":"claim-1","uid":"claim-uid-1","controller":true}]}}`))
		case "/apis/osgym.cua.ai/v1alpha1/namespaces/pool-1/osgymsandboxclaims/claim-1":
			_, _ = w.Write([]byte(`{"metadata":{"name":"claim-1","uid":"claim-uid-1"},"status":{"phase":"Bound","sandbox":{"name":"sandbox-1"}}}`))
		case "/apis/osgym.cua.ai/v1alpha1/namespaces/pool-1/osgymsandboxwarmpools/pool-1":
			_, _ = w.Write([]byte(`{"metadata":{"name":"pool-1"}}`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	oldClient, oldServer, oldToken := k8sClient, k8sAPIServer, k8sSAToken
	k8sClientOnce = sync.Once{}
	overrideK8sClient(server.Client(), server.URL, "test-token")
	t.Cleanup(func() {
		k8sClientOnce = sync.Once{}
		k8sClient, k8sAPIServer, k8sSAToken = oldClient, oldServer, oldToken
	})

	ctx := context.WithValue(context.Background(), auth.UserKey, &auth.User{ID: "user-1"})
	reader := fleetAttributionFactsReader{handlers: Handlers{}}
	claim, err := reader.ReadBoundClaimForSandbox(ctx, "pool-1", "sandbox-1")
	if err != nil || claim != (BoundClaim{Claim: "claim-1", Sandbox: "sandbox-1", Bound: true, OwnerMatched: true}) {
		t.Fatalf("claim = %#v, err = %v", claim, err)
	}
	exists, err := reader.PoolExists(ctx, "pool-1", "pool-1")
	if err != nil || !exists {
		t.Fatalf("pool exists = %v, err = %v", exists, err)
	}
	if len(paths) != 3 || paths[0] != "/apis/osgym.cua.ai/v1alpha1/namespaces/pool-1/osgymsandboxes/sandbox-1" || paths[1] != "/apis/osgym.cua.ai/v1alpha1/namespaces/pool-1/osgymsandboxclaims/claim-1" || paths[2] != "/apis/osgym.cua.ai/v1alpha1/namespaces/pool-1/osgymsandboxwarmpools/pool-1" {
		t.Fatalf("lookup paths = %#v", paths)
	}
}

func TestQualifiesFleetAttribution(t *testing.T) {
	base := AttributionFacts{AuthenticatedNamespace: true, Claim: BoundClaim{Claim: "claim-1", Sandbox: "sandbox-1", Bound: true, OwnerMatched: true}, Service: "sandbox-1-server", NamespacePoolExists: true, UpstreamStatus: 200, Method: "GET", Route: "/api/svc/ns/svc", Path: "/v1/tools"}
	cases := []struct {
		name   string
		mutate func(*AttributionFacts)
		want   bool
	}{
		{"all conjuncts", func(*AttributionFacts) {}, true},
		{"auth missing", func(f *AttributionFacts) { f.AuthenticatedNamespace = false }, false},
		{"claim missing", func(f *AttributionFacts) { f.Claim.Claim = "" }, false},
		{"owner mismatch", func(f *AttributionFacts) { f.Claim.OwnerMatched = false }, false},
		{"not bound", func(f *AttributionFacts) { f.Claim.Bound = false }, false},
		{"sandbox missing", func(f *AttributionFacts) { f.Claim.Sandbox = "" }, false},
		{"service mismatch", func(f *AttributionFacts) { f.Service = "sandbox-1-mcp" }, false},
		{"pool absent", func(f *AttributionFacts) { f.NamespacePoolExists = false }, false},
		{"199", func(f *AttributionFacts) { f.UpstreamStatus = 199 }, false},
		{"300", func(f *AttributionFacts) { f.UpstreamStatus = 300 }, false},
		{"health probe", func(f *AttributionFacts) { f.Path = "/health" }, false},
		{"readiness probe", func(f *AttributionFacts) { f.Path = "/ready" }, false},
		{"metrics probe", func(f *AttributionFacts) { f.Path = "/metrics" }, false},
		{"health descendant", func(f *AttributionFacts) { f.Path = "/health/details" }, false},
		{"metrics descendant", func(f *AttributionFacts) { f.Path = "/metrics/prometheus" }, false},
		{"readiness descendant", func(f *AttributionFacts) { f.Path = "/readiness/dependencies" }, false},
		{"liveness descendant", func(f *AttributionFacts) { f.Path = "/liveness/details" }, false},
		{"ordinary healthy word", func(f *AttributionFacts) { f.Path = "/healthy-recipes" }, true},
		{"ordinary metrics word", func(f *AttributionFacts) { f.Path = "/metrics-report" }, true},
		{"head", func(f *AttributionFacts) { f.Method = "HEAD" }, false},
		{"options", func(f *AttributionFacts) { f.Method = "OPTIONS" }, false},
		{"upgrade", func(f *AttributionFacts) { f.Upgrade = true }, false},
		{"k8s route", func(f *AttributionFacts) { f.Route = "/api/k8s" }, false},
		{"orch route", func(f *AttributionFacts) { f.Route = "/api/orch" }, false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			f := base
			tc.mutate(&f)
			if got := QualifiesFleetAttribution(f); got != tc.want {
				t.Fatalf("got %v want %v", got, tc.want)
			}
		})
	}
}

func TestQualifySvcRequestDerivesExactBindingWithoutClientHeader(t *testing.T) {
	reader := &fakeAttributionFactsReader{claim: BoundClaim{Claim: "claim-1", Sandbox: "sandbox-1", Bound: true, OwnerMatched: true}, pool: true}
	r := httptest.NewRequest("GET", "/api/svc/pool-1/sandbox-1-server/tools", nil)
	r.Pattern = "/api/svc/{namespace}/{service}/{path...}"
	r.SetPathValue("namespace", "pool-1")
	r.SetPathValue("service", "sandbox-1-server")
	r.SetPathValue("path", "tools")
	if !QualifySvcRequest(r.Context(), r, 204, reader) {
		t.Fatal("expected backend-derived binding to qualify")
	}

	for _, mutate := range []func(*http.Request){
		func(r *http.Request) { r.SetPathValue("service", "sandbox-1-mcp") },
		func(r *http.Request) { r.Pattern = "/api/k8s/{path...}" },
	} {
		before := reader.calls
		candidate := r.Clone(r.Context())
		candidate.Header = r.Header.Clone()
		mutate(candidate)
		if QualifySvcRequest(candidate.Context(), candidate, 204, reader) {
			t.Fatal("unexpected qualification")
		}
		if reader.calls != before {
			t.Fatal("invalid or non-svc request reached facts reader")
		}
	}
	if QualifySvcRequest(r.Context(), r, 204, nil) {
		t.Fatal("nil reader must be default-off")
	}
}

func TestQualifySvcRequestResultReportsBoundedReasons(t *testing.T) {
	reader := &fakeAttributionFactsReader{claim: BoundClaim{Claim: "claim-1", Sandbox: "sandbox-1", Bound: true, OwnerMatched: true}, pool: true}
	r := httptest.NewRequest("GET", "/api/svc/pool-1/sandbox-1-server/health", nil)
	r.Pattern = "/api/svc/{namespace}/{service}/{path...}"
	r.SetPathValue("namespace", "pool-1")
	r.SetPathValue("service", "sandbox-1-server")
	r.SetPathValue("path", "health")
	result := QualifySvcRequestResult(r.Context(), r, http.StatusOK, reader)
	if result.Qualifies || result.Reason != "probe_request" {
		t.Fatalf("result = %#v", result)
	}
}

func TestQualifySvcRequestRejectsBindingThatDoesNotMatchSandboxOwner(t *testing.T) {
	reader := &fakeAttributionFactsReader{claim: BoundClaim{Claim: "claim-1", Sandbox: "sandbox-1", Bound: true}, pool: true}
	r := httptest.NewRequest(http.MethodPost, "/api/svc/pool-1/sandbox-1-server/tools", nil)
	r.Pattern = "/api/svc/{namespace}/{service}/{path...}"
	r.SetPathValue("namespace", "pool-1")
	r.SetPathValue("service", "sandbox-1-server")
	r.SetPathValue("path", "tools")
	result := QualifySvcRequestResult(r.Context(), r, http.StatusOK, reader)
	if result.Qualifies || result.Reason != "claim_mismatch" {
		t.Fatalf("result = %#v", result)
	}
}

func TestQualifySvcRequestRejectsUpgradeSignalsBeforeReadingFacts(t *testing.T) {
	for _, headers := range []http.Header{
		{"Upgrade": {"websocket"}},
		{"Connection": {"keep-alive, Upgrade"}},
		{"Connection": {"UPGRADE"}},
	} {
		reader := &fakeAttributionFactsReader{claim: BoundClaim{Claim: "claim-1", Sandbox: "sandbox-1", Bound: true, OwnerMatched: true}, pool: true}
		r := httptest.NewRequest("GET", "/api/svc/pool-1/sandbox-1-server/tools", nil)
		r.Pattern = "/api/svc/{namespace}/{service}/{path...}"
		r.SetPathValue("namespace", "pool-1")
		r.SetPathValue("service", "sandbox-1-server")
		r.SetPathValue("path", "tools")
		r.Header = headers
		if QualifySvcRequest(r.Context(), r, 200, reader) {
			t.Fatal("upgrade request qualified")
		}
		if reader.calls != 0 {
			t.Fatal("upgrade request reached facts reader")
		}
	}
}
