package statequery

import (
	"context"
	"net"
	"testing"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/codes"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/sdk/trace/tracetest"
)

func TestExecutorTracesConnectionFailure(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	address := listener.Addr().String()
	_ = listener.Close()
	recorder := tracetest.NewSpanRecorder()
	provider := sdktrace.NewTracerProvider(sdktrace.WithSpanProcessor(recorder))
	previous := otel.GetTracerProvider()
	otel.SetTracerProvider(provider)
	t.Cleanup(func() { otel.SetTracerProvider(previous); _ = provider.Shutdown(context.Background()) })
	executor, err := NewExecutor("postgres://"+address+"/cyclops?sslmode=disable&connect_timeout=1", "secret")
	if err != nil {
		t.Fatal(err)
	}
	ctx, parent := provider.Tracer("test").Start(context.Background(), "request")
	err = executor.Execute(ctx, "user-alice", "select 'sensitive-query'", &collectingResultWriter{})
	parent.End()
	if err == nil {
		t.Fatal("expected connection failure")
	}
	for _, name := range []string{"db.execute", "db.connect"} {
		found := false
		for _, span := range recorder.Ended() {
			if span.Name() != name {
				continue
			}
			found = true
			if span.Status().Code != codes.Error {
				t.Errorf("%s did not report error", name)
			}
			if !span.Parent().IsValid() {
				t.Errorf("%s missing parent", name)
			}
			for _, attr := range span.Attributes() {
				if string(attr.Key) == "db.statement" || string(attr.Key) == "db.query.text" {
					t.Error("query text exposed")
				}
			}
		}
		if !found {
			t.Errorf("missing span %s", name)
		}
	}
}
