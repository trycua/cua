package handlers

import (
	"context"
	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/usage"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"

	"go.opentelemetry.io/otel"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/sdk/trace/tracetest"
)

type fakeUsageProvider struct {
	overviewQuery UsageQuery
	detailQuery   UsagePoolQuery
	overviewErr   error
	detailErr     error
}

func (f *fakeUsageProvider) Overview(_ context.Context, q UsageQuery) (UsageOverviewResponse, error) {
	f.overviewQuery = q
	return UsageOverviewResponse{}, f.overviewErr
}
func (f *fakeUsageProvider) PoolDetail(_ context.Context, q UsagePoolQuery) (UsagePoolDetailResponse, error) {
	f.detailQuery = q
	return UsagePoolDetailResponse{}, f.detailErr
}
func uh(p UsageProvider, e, a bool) Handlers {
	return Handlers{Usage: p, usageAccessEvaluator: func(context.Context, *auth.User) (bool, error) { return e, nil }, adminAccessEvaluator: func(context.Context, *auth.User) (bool, error) { return a, nil }}
}
func TestGetUsageOverviewScopesAndValidates(t *testing.T) {
	p := &fakeUsageProvider{}
	h := uh(p, true, false)
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
	uh(p, true, true).GetUsageOverview(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/overview?timeframe=30d&subject=customer", nil), &auth.User{ID: "admin"}))
	if w.Code != 200 || p.overviewQuery.ActorSubject != "admin" || p.overviewQuery.Subject != "customer" {
		t.Fatalf("%d %#v", w.Code, p.overviewQuery)
	}
}
func TestUsageNonAdminOverrideDeniedAndMissingProvider503(t *testing.T) {
	p := &fakeUsageProvider{}
	w := httptest.NewRecorder()
	uh(p, true, false).GetUsageOverview(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/overview?timeframe=24h&subject=other", nil), &auth.User{ID: "user"}))
	if w.Code != 403 {
		t.Fatalf("status=%d", w.Code)
	}
	w = httptest.NewRecorder()
	uh(nil, true, false).GetUsageOverview(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/overview?timeframe=24h", nil), &auth.User{ID: "user"}))
	if w.Code != 503 {
		t.Fatalf("status=%d", w.Code)
	}
}
func TestUsagePoolValidationAndInterval(t *testing.T) {
	p := &fakeUsageProvider{}
	w := httptest.NewRecorder()
	uh(p, true, false).GetUsagePoolDetail(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/pool?timeframe=24h&pool=pool-a", nil), &auth.User{ID: "user"}))
	if w.Code != 200 || p.detailQuery.Interval != UsageIntervalHour {
		t.Fatalf("%d %#v", w.Code, p.detailQuery)
	}
	w = httptest.NewRecorder()
	uh(p, true, false).GetUsagePoolDetail(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/pool?timeframe=24h&pool=../../x", nil), &auth.User{ID: "user"}))
	if w.Code != 400 {
		t.Fatalf("status=%d", w.Code)
	}
}

func TestUsageProviderErrorsAreRedacted(t *testing.T) {
	t.Run("mandatory source failure", func(t *testing.T) {
		p := &fakeUsageProvider{overviewErr: errors.New("postgres://secret@db.example: source unavailable")}
		w := httptest.NewRecorder()
		uh(p, true, false).GetUsageOverview(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/overview?timeframe=24h", nil), &auth.User{ID: "user"}))
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
		uh(p, true, false).GetUsagePoolDetail(w, withUser(httptest.NewRequest(http.MethodGet, "/api/usage/pool?timeframe=24h&pool=pool-a", nil), &auth.User{ID: "user"}))
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

	uh(&fakeUsageProvider{}, true, false).GetUsagePoolDetail(response, request)
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
