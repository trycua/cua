package handlers

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/productanalytics"
)

type attributionRoundTripper func(*http.Request) (*http.Response, error)

func (roundTrip attributionRoundTripper) RoundTrip(r *http.Request) (*http.Response, error) {
	return roundTrip(r)
}

func TestQualificationLookupErrorClassesAreBounded(t *testing.T) {
	for _, tc := range []struct {
		name string
		err  error
		want string
	}{
		{"deadline", fmt.Errorf("private-resource: %w", context.DeadlineExceeded), "deadline_exceeded"},
		{"cancel", &url.Error{URL: "https://private.example.test/token", Err: context.Canceled}, "canceled"},
		{"network timeout", &net.DNSError{Err: "private-host", IsTimeout: true}, "timeout"},
		{"request failure", errors.New("secret user@example.test"), "request_failed"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got := classifyAttributionLookupError("claim", tc.err, "request_failed")
			if got.stage != "claim" || got.class != tc.want || got.Error() != "qualification lookup: claim: "+tc.want {
				t.Fatalf("unexpected diagnostic: %#v", got)
			}
		})
	}
	result := attributionLookupRejection("binding_lookup_failed", "unknown", errors.New("private-resource"))
	if result.Qualifies || result.LookupStage != "unknown" || result.LookupErrorClass != "unknown" {
		t.Fatalf("untyped error must not invent a cause: %#v", result)
	}
}

func TestQualificationLookupDiagnosticsPreserveFailClosedChecks(t *testing.T) {
	for _, stage := range []string{"sandbox", "claim", "pool"} {
		for _, tc := range []struct {
			name   string
			status int
			body   string
			err    error
			class  string
		}{
			{name: "unauthenticated", status: 401, class: "unauthenticated"},
			{name: "forbidden", status: 403, class: "forbidden"},
			{name: "missing", status: 404, class: "not_found"},
			{name: "throttled", status: 429, class: "rate_limited"},
			{name: "unavailable", status: 503, class: "upstream_5xx"},
			{name: "unexpected status", status: 400, class: "unexpected_status"},
			{name: "invalid response", status: 200, body: `invalid user@example.test`, class: "invalid_response"},
			{name: "deadline", err: context.DeadlineExceeded, class: "deadline_exceeded"},
			{name: "transport", err: errors.New("private host token"), class: "request_failed"},
		} {
			if stage == "pool" && tc.name == "invalid response" {
				continue // Pool existence checks status only; preserve that contract.
			}
			t.Run(stage+"/"+tc.name, func(t *testing.T) {
				calls := 0
				installAttributionTransport(t, attributionRoundTripper(func(r *http.Request) (*http.Response, error) {
					calls++
					if r.Header.Get("Impersonate-User") == "" || r.Header.Get("Impersonate-Group") == "" {
						t.Error("lookup omitted namespace-scoping impersonation")
					}
					lookupStage, body := attributionLookupFixture(r.URL.Path)
					status := http.StatusOK
					if lookupStage == stage {
						if err := tc.err; err != nil {
							return nil, err
						}
						status, body = tc.status, tc.body
					}
					return &http.Response{StatusCode: status, Body: io.NopCloser(strings.NewReader(body)), Header: make(http.Header)}, nil
				}))
				r := attributionDiagnosticRequest()
				result := (Handlers{}).FleetAttributionQualification()(r.Context(), r, http.StatusOK)
				wantReason, wantCalls := "binding_lookup_failed", 1
				if stage == "claim" {
					wantCalls = 2
				} else if stage == "pool" {
					wantReason, wantCalls = "pool_lookup_failed", 3
				}
				if stage == "pool" && tc.status == 404 {
					if result.Qualifies || result.Reason != "pool_missing" || result.LookupStage != "" || result.LookupErrorClass != "" {
						t.Fatalf("pool absence semantics changed: %#v", result)
					}
					return
				}
				if result.Qualifies || result.Reason != wantReason || result.LookupStage != stage || result.LookupErrorClass != tc.class || calls != wantCalls {
					t.Fatalf("result=%#v calls=%d", result, calls)
				}
				if err := productanalytics.ValidateEvent(productanalytics.Event{Name: productanalytics.EventQualificationRejected, DistinctID: "synthetic-subject", Properties: map[string]any{
					"reason": result.Reason, "qualification_lookup_stage": result.LookupStage, "qualification_error_class": result.LookupErrorClass,
				}}); err != nil {
					t.Fatalf("bounded diagnostic was rejected: %v", err)
				}
			})
		}
	}
}

// A successful workload response is not enough: reproduce a shared fact-read
// deadline expiring at each stage without claiming this is the production cause.
func TestQualificationSharedDeadlineIdentifiesFailingStage(t *testing.T) {
	for _, stage := range []string{"sandbox", "claim", "pool"} {
		t.Run(stage, func(t *testing.T) {
			installAttributionTransport(t, attributionRoundTripper(func(r *http.Request) (*http.Response, error) {
				lookupStage, body := attributionLookupFixture(r.URL.Path)
				if lookupStage == stage {
					<-r.Context().Done()
					return nil, r.Context().Err()
				}
				return &http.Response{StatusCode: http.StatusOK, Body: io.NopCloser(strings.NewReader(body)), Header: make(http.Header)}, nil
			}))
			r := attributionDiagnosticRequest()
			ctx, cancel := context.WithTimeout(r.Context(), 250*time.Millisecond)
			defer cancel()
			result := (Handlers{}).FleetAttributionQualification()(ctx, r, http.StatusOK)
			if result.Qualifies || result.LookupStage != stage || result.LookupErrorClass != "deadline_exceeded" {
				t.Fatalf("result = %#v", result)
			}
		})
	}
}

func TestAttributionReaderPreservesErrorChainButVerdictContainsOnlySafeFields(t *testing.T) {
	for _, stage := range []string{"sandbox", "claim", "pool"} {
		t.Run(stage, func(t *testing.T) {
			cause := &net.DNSError{Name: "private-host.example.test", Err: "private transport detail", IsTimeout: true}
			installAttributionTransport(t, attributionRoundTripper(func(r *http.Request) (*http.Response, error) {
				lookupStage, body := attributionLookupFixture(r.URL.Path)
				if lookupStage == stage {
					return nil, cause
				}
				return &http.Response{StatusCode: http.StatusOK, Body: io.NopCloser(strings.NewReader(body)), Header: make(http.Header)}, nil
			}))
			r := attributionDiagnosticRequest()
			reader := fleetAttributionFactsReader{handlers: Handlers{}}
			var err error
			if stage == "pool" {
				_, err = reader.PoolExists(r.Context(), "pool-1", "pool-1")
			} else {
				_, err = reader.ReadBoundClaimForSandbox(r.Context(), "pool-1", "sandbox-1")
			}
			var original *net.DNSError
			var diagnostic attributionLookupError
			if !errors.Is(err, cause) || !errors.As(err, &original) || original != cause || !errors.As(err, &diagnostic) {
				t.Fatal("reader must preserve original error identity and typed diagnostic")
			}
			if diagnostic.stage != stage || diagnostic.class != "timeout" {
				t.Fatalf("diagnostic = %#v", diagnostic)
			}
			result := QualifySvcRequestResult(r.Context(), r, http.StatusOK, reader)
			wantReason := "binding_lookup_failed"
			if stage == "pool" {
				wantReason = "pool_lookup_failed"
			}
			if result != (QualificationResult{Reason: wantReason, LookupStage: stage, LookupErrorClass: "timeout"}) {
				t.Fatalf("verdict must contain only bounded diagnostic fields: %#v", result)
			}
		})
	}
}

func installAttributionTransport(t *testing.T, transport http.RoundTripper) {
	t.Helper()
	oldClient, oldServer, oldToken := k8sClient, k8sAPIServer, k8sSAToken
	k8sClientOnce = sync.Once{}
	overrideK8sClient(&http.Client{Transport: transport}, "http://kubernetes.example.test", "synthetic-token")
	t.Cleanup(func() {
		k8sClientOnce = sync.Once{}
		k8sClient, k8sAPIServer, k8sSAToken = oldClient, oldServer, oldToken
	})
}

func attributionDiagnosticRequest() *http.Request {
	r := httptest.NewRequest(http.MethodPost, "/api/svc/pool-1/sandbox-1-server/tools", nil)
	r.Pattern = "/api/svc/{namespace}/{service}/{path...}"
	r.SetPathValue("namespace", "pool-1")
	r.SetPathValue("service", "sandbox-1-server")
	r.SetPathValue("path", "tools")
	return r.WithContext(context.WithValue(r.Context(), auth.UserKey, &auth.User{ID: "synthetic-subject"}))
}

func attributionLookupFixture(path string) (string, string) {
	switch {
	case strings.Contains(path, "/osgymsandboxes/"):
		return "sandbox", `{"metadata":{"ownerReferences":[{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","name":"claim-1","uid":"claim-uid-1","controller":true}]}}`
	case strings.Contains(path, "/osgymsandboxclaims/"):
		return "claim", `{"metadata":{"name":"claim-1","uid":"claim-uid-1"},"status":{"phase":"Bound","sandbox":{"name":"sandbox-1"}}}`
	default:
		return "pool", `{}`
	}
}
