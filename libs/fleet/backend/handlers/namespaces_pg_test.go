package handlers

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"cyclops-cs-backend/statequery"
	"github.com/open-feature/go-sdk/openfeature"

	"cyclops-cs-backend/auth"
	"github.com/trycua/cloud/pkg/featureflags"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/sdk/trace/tracetest"
)

func TestListNamespacesPostgresFlag(t *testing.T) {
	for _, test := range []struct {
		name, flag string
		executor   *fakeStateQueryExecutor
		want       string
		k8sCalls   int
	}{
		{"disabled", "false", &fakeStateQueryExecutor{}, "k8s-ns", 1},
		{"missing flag", "", &fakeStateQueryExecutor{}, "k8s-ns", 1},
		{"postgres", "true", &fakeStateQueryExecutor{rows: [][]any{{`{"metadata":{"name":"pg-ns","labels":{"test":"value"}},"status":{"phase":"Active"}}`}}}, "pg-ns", 0},
		{"empty success", "true", &fakeStateQueryExecutor{}, "", 0},
		{"unavailable", "true", nil, "k8s-ns", 1},
		{"partial failure", "true", &fakeStateQueryExecutor{rows: [][]any{{`{"metadata":{"name":"partial"}}`}}, err: errors.New("query failed")}, "k8s-ns", 1},
		{"invalid JSON", "true", &fakeStateQueryExecutor{rows: [][]any{{`broken`}}}, "k8s-ns", 1},
		{"invalid column", "true", &fakeStateQueryExecutor{rows: [][]any{{123}}}, "k8s-ns", 1},
		{"invalid column count", "true", &fakeStateQueryExecutor{rows: [][]any{{}}}, "k8s-ns", 1},
		{"missing namespace name", "true", &fakeStateQueryExecutor{rows: [][]any{{`{}`}}}, "k8s-ns", 1},
	} {
		t.Run(test.name, func(t *testing.T) {
			t.Setenv("CYCLOPS_CS_FF_LIST_NS_READ_PG", test.flag)
			if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
				t.Fatal(err)
			}
			proxy := newFakeK8s(http.StatusOK, nsListResponse("k8s-ns"))
			defer proxy.server.Close()
			overrideK8sClient(proxy.server.Client(), proxy.server.URL, "token")
			handler := Handlers{}
			if test.executor != nil {
				handler.Features = FeaturesWith(test.executor, nil)
			}
			response := httptest.NewRecorder()
			handler.ListNamespaces(response, withUser(httptest.NewRequest("GET", "/api/namespaces", nil), &auth.User{ID: "alice"}))
			if response.Code != 200 {
				t.Fatalf("status %d: %s", response.Code, response.Body.String())
			}
			var namespaces []NamespaceResponse
			if err := json.Unmarshal(response.Body.Bytes(), &namespaces); err != nil {
				t.Fatal(err)
			}
			if test.want == "" {
				if namespaces == nil || len(namespaces) != 0 {
					t.Fatalf("want [], got %s", response.Body.String())
				}
			} else if len(namespaces) != 1 || namespaces[0].Name != test.want {
				t.Fatalf("namespaces = %+v", namespaces)
			}
			if len(proxy.requests) != test.k8sCalls {
				t.Fatalf("Kubernetes calls = %d, want %d", len(proxy.requests), test.k8sCalls)
			}
			if test.executor != nil {
				if test.flag == "true" && test.executor.tenant != "user-alice" {
					t.Fatalf("tenant = %q", test.executor.tenant)
				}
				if test.flag != "true" && test.executor.query != "" {
					t.Fatal("queried PostgreSQL with flag off")
				}
			}
		})
	}
}

func TestListNamespacesPostgresFilteringAndTracing(t *testing.T) {
	t.Setenv("CYCLOPS_CS_FF_LIST_NS_READ_PG", "true")
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatal(err)
	}
	recorder := tracetest.NewSpanRecorder()
	provider := sdktrace.NewTracerProvider(sdktrace.WithSpanProcessor(recorder))
	previous := otel.GetTracerProvider()
	otel.SetTracerProvider(provider)
	t.Cleanup(func() { otel.SetTracerProvider(previous); _ = provider.Shutdown(context.Background()) })
	executor := &fakeStateQueryExecutor{rows: [][]any{{`{"metadata":{"name":"allowed"}}`}, {`{"metadata":{"name":"denied"}}`}}}
	handler := Handlers{Features: FeaturesWith(executor, nil)}
	response := httptest.NewRecorder()
	handler.ListNamespaces(response, withUser(httptest.NewRequest("GET", "/api/namespaces", nil), &auth.User{ID: "alice", PrincipalType: auth.PrincipalTypeGitHubOIDC, AllowedNamespaces: []string{"allowed"}}))
	var namespaces []NamespaceResponse
	if err := json.Unmarshal(response.Body.Bytes(), &namespaces); err != nil {
		t.Fatal(err)
	}
	if len(namespaces) != 1 || namespaces[0].Name != "allowed" {
		t.Fatalf("namespaces = %+v", namespaces)
	}
	for _, name := range []string{"namespaces.list", "feature_flag.evaluate", "identity.resolve_tenant", "namespaces.read.postgres", "namespaces.filter", "response.encode_write"} {
		found := false
		for _, span := range recorder.Ended() {
			if span.Name() == name {
				found = true
				if name != "namespaces.list" && !span.Parent().IsValid() {
					t.Fatalf("%s missing parent", name)
				}
			}
		}
		if !found {
			t.Errorf("missing span %s", name)
		}
	}
}

func TestListNamespacesFallbackTracing(t *testing.T) {
	for _, status := range []int{http.StatusOK, http.StatusForbidden} {
		t.Run(http.StatusText(status), func(t *testing.T) {
			t.Setenv("CYCLOPS_CS_FF_LIST_NS_READ_PG", "true")
			if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
				t.Fatal(err)
			}
			proxy := newFakeK8s(status, nsListResponse("k8s-ns"))
			defer proxy.server.Close()
			overrideK8sClient(proxy.server.Client(), proxy.server.URL, "token")
			recorder := tracetest.NewSpanRecorder()
			provider := sdktrace.NewTracerProvider(sdktrace.WithSpanProcessor(recorder))
			previous := otel.GetTracerProvider()
			otel.SetTracerProvider(provider)
			t.Cleanup(func() { otel.SetTracerProvider(previous); _ = provider.Shutdown(context.Background()) })
			handler := Handlers{Features: FeaturesWith(&fakeStateQueryExecutor{err: errors.New("pg failed")}, nil)}
			response := httptest.NewRecorder()
			handler.ListNamespaces(response, withUser(httptest.NewRequest("GET", "/api/namespaces", nil), &auth.User{ID: "alice"}))
			if response.Code != status {
				t.Fatalf("status = %d, want %d", response.Code, status)
			}
			spans := map[string]sdktrace.ReadOnlySpan{}
			for _, span := range recorder.Ended() {
				spans[span.Name()] = span
			}
			for _, name := range []string{"namespaces.list", "namespaces.read.postgres", "namespaces.read.kubernetes", "kubernetes.request"} {
				if spans[name] == nil {
					t.Fatalf("missing span %s", name)
				}
			}
			parent := spans["namespaces.list"]
			if spans["namespaces.read.postgres"].Status().Code != codes.Error {
				t.Error("PG failure not marked")
			}
			if status == http.StatusOK && parent.Status().Code == codes.Error {
				t.Error("successful fallback marked as error")
			}
			if status != http.StatusOK && parent.Status().Code != codes.Error {
				t.Error("failed fallback not marked as error")
			}
			attrs := map[string]attribute.Value{}
			for _, attr := range parent.Attributes() {
				attrs[string(attr.Key)] = attr.Value
			}
			if !attrs["read.fallback"].AsBool() || attrs["read.backend"].AsString() != "kubernetes" {
				t.Fatalf("fallback attributes = %v", attrs)
			}
			if spans["namespaces.read.postgres"].Parent().SpanID() != parent.SpanContext().SpanID() || spans["namespaces.read.kubernetes"].Parent().SpanID() != parent.SpanContext().SpanID() {
				t.Error("read attempts must be siblings")
			}
			foundFirstByte := false
			for _, event := range spans["kubernetes.request"].Events() {
				if event.Name == "http.response.first_byte" {
					foundFirstByte = true
				}
			}
			if !foundFirstByte {
				t.Error("missing network timing event")
			}
		})
	}
}

type namespaceFlagErrorProvider struct{ featureflags.SimpleEnvProvider }

func (namespaceFlagErrorProvider) BooleanEvaluation(context.Context, string, bool, openfeature.FlattenedContext) openfeature.BoolResolutionDetail {
	return openfeature.BoolResolutionDetail{Value: true, ProviderResolutionDetail: openfeature.ProviderResolutionDetail{ResolutionError: openfeature.NewGeneralResolutionError("flag provider unavailable")}}
}

func TestListNamespacesFlagErrorUsesKubernetes(t *testing.T) {
	if err := openfeature.SetProviderAndWait(&namespaceFlagErrorProvider{}); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = openfeature.SetProviderAndWait(&featureflags.SimpleEnvProvider{}) })
	proxy := newFakeK8s(http.StatusOK, nsListResponse("k8s-ns"))
	defer proxy.server.Close()
	overrideK8sClient(proxy.server.Client(), proxy.server.URL, "token")
	executor := &fakeStateQueryExecutor{}
	handler := Handlers{Features: FeaturesWith(executor, nil)}
	response := httptest.NewRecorder()
	handler.ListNamespaces(response, withUser(httptest.NewRequest("GET", "/api/namespaces", nil), &auth.User{ID: "alice"}))
	if response.Code != http.StatusOK || len(proxy.requests) != 1 || executor.query != "" {
		t.Fatalf("flag error did not use Kubernetes: status=%d requests=%d query=%q", response.Code, len(proxy.requests), executor.query)
	}
}

type namespaceDeadlineExecutor struct{ testing *testing.T }

func (executor namespaceDeadlineExecutor) Execute(ctx context.Context, _, _ string, _ statequery.ResultWriter) error {
	deadline, ok := ctx.Deadline()
	if !ok || time.Until(deadline) > namespacePostgresTimeout {
		executor.testing.Fatal("PostgreSQL attempt is not bounded")
	}
	<-ctx.Done()
	return ctx.Err()
}

func TestListNamespacesDeadlineFallsBackWithLiveContext(t *testing.T) {
	t.Setenv("CYCLOPS_CS_FF_LIST_NS_READ_PG", "true")
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatal(err)
	}
	proxy := newFakeK8s(http.StatusOK, nsListResponse("k8s-ns"))
	defer proxy.server.Close()
	overrideK8sClient(proxy.server.Client(), proxy.server.URL, "token")
	handler := Handlers{Features: FeaturesWith(namespaceDeadlineExecutor{testing: t}, nil)}
	response := httptest.NewRecorder()
	handler.ListNamespaces(response, withUser(httptest.NewRequest("GET", "/api/namespaces", nil), &auth.User{ID: "alice"}))
	if response.Code != http.StatusOK || len(proxy.requests) != 1 {
		t.Fatalf("deadline fallback failed: status=%d requests=%d", response.Code, len(proxy.requests))
	}
}

func TestNamespaceReadErrorsPreserveCauses(t *testing.T) {
	writer := &namespaceResultWriter{}
	var syntaxErr *json.SyntaxError
	if err := writer.WriteRow([]any{"invalid"}); !errors.As(err, &syntaxErr) || err.Error() != "namespace query returned invalid object" {
		t.Fatalf("namespace row decode must preserve a redacted cause: %v", err)
	}
	proxy := newFakeK8s(http.StatusOK, "invalid")
	defer proxy.server.Close()
	overrideK8sClient(proxy.server.Client(), proxy.server.URL, "token")
	handler := Handlers{}
	_, status, err := handler.readNamespacesK8s(context.Background(), "user-alice", "alice")
	if status != http.StatusBadGateway || !errors.As(err, &syntaxErr) || err.Error() != "bad response from k8s" {
		t.Fatalf("Kubernetes decode must preserve a redacted cause: %d %v", status, err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, status, err = handler.readNamespacesK8s(ctx, "user-alice", "alice")
	if status != http.StatusBadGateway || !errors.Is(err, context.Canceled) || err.Error() != "kubectl-proxy unavailable" {
		t.Fatalf("Kubernetes transport must preserve a redacted cause: %d %v", status, err)
	}
}
