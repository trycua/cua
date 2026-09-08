package usage

import (
	"context"
	"sync"
	"time"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/metric"
)

var (
	usageMetricsOnce sync.Once
	cacheRequests    metric.Int64Counter
	loadDuration     metric.Float64Histogram
)

func initUsageMetrics() {
	meter := otel.Meter("cyclops-cs-backend/usage")
	cacheRequests, _ = meter.Int64Counter(
		"cyclops.usage.cache.requests",
		metric.WithDescription("Usage provider cache lookups by outcome."),
	)
	loadDuration, _ = meter.Float64Histogram(
		"cyclops.usage.load.duration",
		metric.WithDescription("Duration of uncached Usage provider loads."),
		metric.WithUnit("s"),
	)
}

func recordUsageCache(ctx context.Context, timeframe Timeframe, outcome string) {
	usageMetricsOnce.Do(initUsageMetrics)
	cacheRequests.Add(ctx, 1, metric.WithAttributes(
		attribute.String("usage.timeframe", string(timeframe)),
		attribute.String("usage.cache.outcome", outcome),
	))
}

func recordUsageLoad(ctx context.Context, timeframe Timeframe, duration time.Duration, err error) {
	usageMetricsOnce.Do(initUsageMetrics)
	outcome := "success"
	if err != nil {
		outcome = "error"
	}
	loadDuration.Record(ctx, duration.Seconds(), metric.WithAttributes(
		attribute.String("usage.timeframe", string(timeframe)),
		attribute.String("usage.load.outcome", outcome),
	))
}
