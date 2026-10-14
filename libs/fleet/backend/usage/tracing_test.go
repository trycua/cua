package usage

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/sdk/trace/tracetest"
)

func TestProviderPoolDetailCreatesLatencyBreakdownSpans(t *testing.T) {
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	recorder := tracetest.NewSpanRecorder()
	provider := sdktrace.NewTracerProvider(sdktrace.WithSpanProcessor(recorder))
	previousProvider := otel.GetTracerProvider()
	otel.SetTracerProvider(provider)
	t.Cleanup(func() { otel.SetTracerProvider(previousProvider) })

	ctx, root := provider.Tracer("test").Start(context.Background(), "request")
	usageProvider := NewProvider(
		&fakeEventStore{events: []SandboxEvent{{
			EventID: mustUUID("00000000-0000-0000-0000-000000000101"), Namespace: "ns-a", SandboxName: "sandbox-a", SandboxUID: "uid-a",
			PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm-a", EventType: "Added", ObservedAt: cutoff.Add(-25 * time.Hour),
		}}},
		&fakeAllocationClient{asOf: cutoff, allocations: []Allocation{{
			Start: cutoff.Add(-time.Hour), End: cutoff, Namespace: "ns-a", Pod: "virt-launcher-vm-a-abc",
			Minutes: 60, CPUUsageAverage: 2, CPURequestAverage: 4,
			RAMUsageAverageBytes: 3 * gibibyte, RAMRequestAverageBytes: 6 * gibibyte,
		}}},
		func() time.Time { return cutoff },
	)

	_, err := usageProvider.PoolDetail(ctx, PoolQuery{
		Query:    Query{Subject: "user-should-not-appear", Timeframe: Timeframe24H},
		PoolID:   "ns-a:pool-a",
		Interval: IntervalHour,
	})
	root.End()
	if err != nil {
		t.Fatal(err)
	}

	for _, name := range []string{
		"usage.load",
		"usage.events.query",
		"usage.segments.build",
		"usage.allocations.query",
		"usage.allocations.attribute",
		"usage.buckets.build",
	} {
		if findEndedSpan(recorder.Ended(), name) == nil {
			t.Fatalf("expected %q span", name)
		}
	}

	loadSpan := findEndedSpan(recorder.Ended(), "usage.load")
	assertSpanAttribute(t, loadSpan.Attributes(), "usage.timeframe", "24h")
	assertSpanAttribute(t, findEndedSpan(recorder.Ended(), "usage.events.query").Attributes(), "usage.event_count", int64(1))
	assertSpanAttribute(t, findEndedSpan(recorder.Ended(), "usage.allocations.query").Attributes(), "usage.allocation_count", int64(1))
	assertSpanAttribute(t, findEndedSpan(recorder.Ended(), "usage.buckets.build").Attributes(), "usage.bucket_count", int64(24))

	for _, span := range recorder.Ended() {
		for _, attr := range span.Attributes() {
			if attr.Value.AsString() == "user-should-not-appear" || attr.Value.AsString() == "ns-a:pool-a" {
				t.Fatalf("span %q contains sensitive/high-cardinality attribute %q", span.Name(), attr.Key)
			}
		}
	}
}

func findEndedSpan(spans []sdktrace.ReadOnlySpan, name string) sdktrace.ReadOnlySpan {
	for _, span := range spans {
		if span.Name() == name {
			return span
		}
	}
	return nil
}

func assertSpanAttribute(t *testing.T, attributes []attribute.KeyValue, key string, want any) {
	t.Helper()
	for _, attr := range attributes {
		if string(attr.Key) == key {
			if got := attr.Value.AsInterface(); got != want {
				t.Fatalf("attribute %s = %#v, want %#v", key, got, want)
			}
			return
		}
	}
	t.Fatalf("missing attribute %s", key)
}

func TestProviderErrorSpansDoNotExportSourceMessages(t *testing.T) {
	recorder := tracetest.NewSpanRecorder()
	provider := sdktrace.NewTracerProvider(sdktrace.WithSpanProcessor(recorder))
	previousProvider := otel.GetTracerProvider()
	otel.SetTracerProvider(provider)
	t.Cleanup(func() { otel.SetTracerProvider(previousProvider) })

	ctx, root := provider.Tracer("test").Start(context.Background(), "request")
	_, err := NewProvider(
		&fakeEventStore{err: errors.New("postgres://secret@db.example: source unavailable")},
		&fakeAllocationClient{},
		func() time.Time { return time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC) },
	).Overview(ctx, Query{Subject: "user", Timeframe: Timeframe24H})
	root.End()
	if err == nil {
		t.Fatal("expected source error")
	}

	span := findEndedSpan(recorder.Ended(), "usage.events.query")
	if span == nil {
		t.Fatal("expected usage.events.query span")
	}
	if span.Status().Code != codes.Error {
		t.Fatalf("span status = %v, want error", span.Status().Code)
	}
	for _, event := range span.Events() {
		for _, attr := range event.Attributes {
			if strings.Contains(fmt.Sprint(attr.Value.AsInterface()), "secret@db.example") {
				t.Fatalf("span exported source error through %q", attr.Key)
			}
		}
	}
}
