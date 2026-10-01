package handlers

import (
	"context"
	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/usage"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"go.opentelemetry.io/otel"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/sdk/trace/tracetest"
)

type fakeUsageProvider struct {
	overviewQuery UsageQuery
	detailQuery   UsagePoolQuery
	overviewCalls int
	detailCalls   int
	overviewErr   error
	detailErr     error
}

func (f *fakeUsageProvider) Overview(_ context.Context, q UsageQuery) (UsageOverviewResponse, error) {
	f.overviewCalls++
	f.overviewQuery = q
	return UsageOverviewResponse{}, f.overviewErr
}
func (f *fakeUsageProvider) PoolDetail(_ context.Context, q UsagePoolQuery) (UsagePoolDetailResponse, error) {
	f.detailCalls++
	f.detailQuery = q
	return UsagePoolDetailResponse{}, f.detailErr
}
func uh(p UsageProvider, admin bool) Handlers {
	return Handlers{Usage: p, adminAccessEvaluator: func(context.Context, *auth.User) (bool, error) { return admin, nil }}
}
func TestGetUsageOverviewScopesAndValidates(t *testing.T) {
	p := &fakeUsageProvider{}
	h := uh(p, false)
	w := httptest.NewRecorder()
	h.GetUsageOverview(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/overview?timeframe=7d", nil), &auth.User{ID: "user"}))
	if w.Code != 200 || p.overviewQuery.Subject != "user" || p.overviewQuery.ActorSubject != "user" {
		t.Fatalf("status/query: %d %#v", w.Code, p.overviewQuery)
	}
	w = httptest.NewRecorder()
	h.GetUsageOverview(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/overview?timeframe=90d", nil), &auth.User{ID: "user"}))
	if w.Code != 400 {
		t.Fatalf("status=%d", w.Code)
	}
}
func TestUsageAdminViewAsPreservesActor(t *testing.T) {
	p := &fakeUsageProvider{}
	w := httptest.NewRecorder()
	uh(p, true).GetUsageOverview(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/overview?timeframe=30d&subject=customer", nil), &auth.User{ID: "admin"}))
	if w.Code != 200 || p.overviewQuery.ActorSubject != "admin" || p.overviewQuery.Subject != "customer" {
		t.Fatalf("%d %#v", w.Code, p.overviewQuery)
	}
}
func TestUsageNonAdminOverrideDeniedBeforeProviderAccess(t *testing.T) {
	p := &fakeUsageProvider{}
	w := httptest.NewRecorder()
	uh(p, false).GetUsageOverview(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/overview?timeframe=24h&subject=other", nil), &auth.User{ID: "user"}))
	if got, want := w.Code, http.StatusForbidden; got != want {
		t.Fatalf("status=%d, want %d", got, want)
	}
	if got, want := w.Body.String(), "{\"error\":\"only administrators can select another subject\"}\n"; got != want {
		t.Fatalf("body=%q, want %q", got, want)
	}
	if p.overviewCalls != 0 {
		t.Fatalf("provider overview calls=%d, want 0", p.overviewCalls)
	}

	w = httptest.NewRecorder()
	uh(p, false).GetUsagePoolDetail(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/pool?timeframe=24h&pool=pool-a&subject=other", nil), &auth.User{ID: "user"}))
	if got, want := w.Code, http.StatusForbidden; got != want {
		t.Fatalf("pool status=%d, want %d", got, want)
	}
	if p.detailCalls != 0 {
		t.Fatalf("provider detail calls=%d, want 0", p.detailCalls)
	}
}

func TestUsageMissingProviderReturns503(t *testing.T) {
	w := httptest.NewRecorder()
	uh(nil, false).GetUsageOverview(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/overview?timeframe=24h", nil), &auth.User{ID: "user"}))
	if w.Code != 503 {
		t.Fatalf("status=%d", w.Code)
	}
}
func TestUsagePoolValidationAndInterval(t *testing.T) {
	p := &fakeUsageProvider{}
	w := httptest.NewRecorder()
	uh(p, false).GetUsagePoolDetail(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/pool?timeframe=24h&pool=pool-a", nil), &auth.User{ID: "user"}))
	if w.Code != 200 || p.detailQuery.Interval != UsageIntervalHour {
		t.Fatalf("%d %#v", w.Code, p.detailQuery)
	}
	w = httptest.NewRecorder()
	uh(p, false).GetUsagePoolDetail(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/pool?timeframe=24h&pool=../../x", nil), &auth.User{ID: "user"}))
	if w.Code != 400 {
		t.Fatalf("status=%d", w.Code)
	}
}

func TestRecordUsageBrowserTimingsValidatesPayload(t *testing.T) {
	handler := uh(&fakeUsageProvider{}, false)
	request := withUser(
		httptest.NewRequest(http.MethodPost, "/api/usage/browser-timings?timeframe=24h", strings.NewReader(`{"initial_load_ms":120,"dashboard_ready_ms":140}`)),
		&auth.User{ID: "user"},
	)
	response := httptest.NewRecorder()
	handler.RecordUsageBrowserTimings(response, request)
	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusNoContent)
	}

	request = withUser(
		httptest.NewRequest(http.MethodPost, "/api/usage/browser-timings?timeframe=24h", strings.NewReader(`{"initial_load_ms":120,"dashboard_ready_ms":10}`)),
		&auth.User{ID: "user"},
	)
	response = httptest.NewRecorder()
	handler.RecordUsageBrowserTimings(response, request)
	if response.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusBadRequest)
	}
}

func TestUsageProviderErrorsAreRedacted(t *testing.T) {
	t.Run("mandatory source failure", func(t *testing.T) {
		p := &fakeUsageProvider{overviewErr: errors.New("postgres://secret@db.example: source unavailable")}
		w := httptest.NewRecorder()
		uh(p, false).GetUsageOverview(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/overview?timeframe=24h", nil), &auth.User{ID: "user"}))
		if w.Code != http.StatusBadGateway {
			t.Fatalf("status = %d, want %d", w.Code, http.StatusBadGateway)
		}
		if got, want := w.Body.String(), "{\"error\":\"usage data is temporarily unavailable\"}\n"; got != want {
			t.Fatalf("body = %q, want %q", got, want)
		}
		if got, want := w.Header().Get("Cache-Control"), "private, no-store"; got != want {
			t.Fatalf("Cache-Control = %q, want %q", got, want)
		}
	})

	t.Run("missing pool", func(t *testing.T) {
		p := &fakeUsageProvider{detailErr: usage.ErrPoolNotFound}
		w := httptest.NewRecorder()
		uh(p, false).GetUsagePoolDetail(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/pool?timeframe=24h&pool=pool-a", nil), &auth.User{ID: "user"}))
		if w.Code != http.StatusNotFound {
			t.Fatalf("status = %d, want %d", w.Code, http.StatusNotFound)
		}
		if got, want := w.Body.String(), "{\"error\":\"usage pool was not found\"}\n"; got != want {
			t.Fatalf("body = %q, want %q", got, want)
		}
	})
}

func TestUsagePoolDetailCreatesHandlerAndAuthorizationSpans(t *testing.T) {
	recorder := tracetest.NewSpanRecorder()
	provider := sdktrace.NewTracerProvider(sdktrace.WithSpanProcessor(recorder))
	previousProvider := otel.GetTracerProvider()
	otel.SetTracerProvider(provider)
	t.Cleanup(func() { otel.SetTracerProvider(previousProvider) })

	ctx, root := provider.Tracer("test").Start(context.Background(), "request")
	request := withUser(
		httptest.NewRequest(http.MethodGet, "/api/usage/pool?timeframe=24h&pool=pool-should-not-appear", nil).WithContext(ctx),
		&auth.User{ID: "user-should-not-appear"},
	)
	response := httptest.NewRecorder()

	uh(&fakeUsageProvider{}, false).GetUsagePoolDetail(response, request)
	root.End()

	if response.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusOK)
	}
	for _, name := range []string{"usage.pool_detail", "usage.authorize"} {
		if handlerSpanByName(recorder.Ended(), name) == nil {
			t.Fatalf("expected %q span", name)
		}
	}
	for _, span := range recorder.Ended() {
		for _, attr := range span.Attributes() {
			if attr.Value.AsString() == "user-should-not-appear" || attr.Value.AsString() == "pool-should-not-appear" {
				t.Fatalf("span %q contains sensitive/high-cardinality attribute %q", span.Name(), attr.Key)
			}
		}
	}
}

func handlerSpanByName(spans []sdktrace.ReadOnlySpan, name string) sdktrace.ReadOnlySpan {
	for _, span := range spans {
		if span.Name() == name {
			return span
		}
	}
	return nil
}
