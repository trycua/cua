package telemetry

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"

	"go.opentelemetry.io/otel"
)

func TestInit_ExportsSpansOverOTLPHTTP(t *testing.T) {
	var (
		mu        sync.Mutex
		bodySizes = map[string]int{}
	)

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, err := io.ReadAll(r.Body)
		if err != nil {
			t.Fatalf("read body: %v", err)
		}
		mu.Lock()
		bodySizes[r.URL.Path] = len(body)
		mu.Unlock()
		w.WriteHeader(http.StatusAccepted)
	}))
	defer srv.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	shutdown, err := Init(ctx, Config{
		Endpoint:         srv.URL,
		Protocol:         "http/protobuf",
		ServiceName:      "cyclops-cs-backend",
		ServiceNamespace: "cyclops-cs",
		Environment:      "production",
	})
	if err != nil {
		t.Fatalf("Init() error = %v", err)
	}
	defer func() {
		if err := shutdown(context.Background()); err != nil {
			t.Fatalf("shutdown() error = %v", err)
		}
	}()

	_, span := otel.Tracer("test").Start(ctx, "backend.request")
	span.End()
	counter, err := otel.Meter("test").Int64Counter("test.requests")
	if err != nil {
		t.Fatalf("create counter: %v", err)
	}
	counter.Add(ctx, 1)

	if err := shutdown(context.Background()); err != nil {
		t.Fatalf("shutdown() error = %v", err)
	}

	mu.Lock()
	defer mu.Unlock()

	for _, path := range []string{"/v1/traces", "/v1/metrics"} {
		if bodySizes[path] == 0 {
			t.Fatalf("body size for %s = %d, want > 0; requests = %#v", path, bodySizes[path], bodySizes)
		}
	}
}
