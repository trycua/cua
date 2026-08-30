package handlers

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
)

type fakeAttributionFactsReader struct {
	claim BoundClaim
	pool  bool
	calls int
}

func (f *fakeAttributionFactsReader) ReadBoundClaim(context.Context, string, string) (BoundClaim, error) {
	f.calls++
	return f.claim, nil
}
func (f *fakeAttributionFactsReader) PoolExists(context.Context, string, string) (bool, error) {
	f.calls++
	return f.pool, nil
}

func TestQualifiesFleetAttribution(t *testing.T) {
	base := AttributionFacts{AuthenticatedNamespace: true, SDKClaim: "claim-1", Claim: BoundClaim{Claim: "claim-1", Sandbox: "sandbox-1", Bound: true}, Service: "sandbox-1-server", NamespacePoolExists: true, UpstreamStatus: 200, Method: "GET", Route: "/api/svc/ns/svc", Path: "/v1/tools"}
	cases := []struct {
		name   string
		mutate func(*AttributionFacts)
		want   bool
	}{
		{"all conjuncts", func(*AttributionFacts) {}, true},
		{"auth missing", func(f *AttributionFacts) { f.AuthenticatedNamespace = false }, false},
		{"sdk claim missing", func(f *AttributionFacts) { f.SDKClaim = "" }, false},
		{"claim mismatch", func(f *AttributionFacts) { f.Claim.Claim = "other" }, false},
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

func TestQualifySvcRequestUsesOnlyValidatedExactClaimAndSvcRoutes(t *testing.T) {
	reader := &fakeAttributionFactsReader{claim: BoundClaim{Claim: "claim-1", Sandbox: "sandbox-1", Bound: true}, pool: true}
	r := httptest.NewRequest("GET", "/api/svc/pool-1/sandbox-1-server/tools", nil)
	r.Pattern = "/api/svc/{namespace}/{service}/{path...}"
	r.SetPathValue("namespace", "pool-1")
	r.SetPathValue("service", "sandbox-1-server")
	r.SetPathValue("path", "tools")
	r.Header.Set(FleetClaimHeader, "claim-1")
	if !QualifySvcRequest(r.Context(), r, 204, reader) {
		t.Fatal("expected exact claim to qualify")
	}

	for _, mutate := range []func(*http.Request){
		func(r *http.Request) { r.Header.Set(FleetClaimHeader, "bad claim") },
		func(r *http.Request) { r.Header.Add(FleetClaimHeader, "claim-2") },
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

func TestQualifySvcRequestRejectsUpgradeSignalsBeforeReadingFacts(t *testing.T) {
	for _, headers := range []http.Header{
		{"Upgrade": {"websocket"}},
		{"Connection": {"keep-alive, Upgrade"}},
		{"Connection": {"UPGRADE"}},
	} {
		reader := &fakeAttributionFactsReader{claim: BoundClaim{Claim: "claim-1", Sandbox: "sandbox-1", Bound: true}, pool: true}
		r := httptest.NewRequest("GET", "/api/svc/pool-1/sandbox-1-server/tools", nil)
		r.Pattern = "/api/svc/{namespace}/{service}/{path...}"
		r.SetPathValue("namespace", "pool-1")
		r.SetPathValue("service", "sandbox-1-server")
		r.SetPathValue("path", "tools")
		r.Header = headers
		r.Header.Set(FleetClaimHeader, "claim-1")
		if QualifySvcRequest(r.Context(), r, 200, reader) {
			t.Fatal("upgrade request qualified")
		}
		if reader.calls != 0 {
			t.Fatal("upgrade request reached facts reader")
		}
	}
}
