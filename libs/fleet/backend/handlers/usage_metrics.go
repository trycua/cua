package handlers

import (
	"context"
	"sync"
	"time"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/metric"
)

var (
	usageBrowserMetricsOnce sync.Once
	usageBrowserDuration    metric.Float64Histogram
)

func initUsageBrowserMetrics() {
	usageBrowserDuration, _ = otel.Meter("cyclops-cs-backend/handlers").Float64Histogram(
		"cyclops.usage.browser.duration",
		metric.WithDescription("Browser-observed Usage dashboard phase duration."),
		metric.WithUnit("s"),
	)
}

func recordUsageBrowserTimings(ctx context.Context, timeframe UsageTimeframe, timings usageBrowserTimings) {
	usageBrowserMetricsOnce.Do(initUsageBrowserMetrics)
	for phase, duration := range map[string]time.Duration{
		"initial_load":    time.Duration(timings.InitialLoadMS * float64(time.Millisecond)),
		"dashboard_ready": time.Duration(timings.DashboardReadyMS * float64(time.Millisecond)),
	} {
		usageBrowserDuration.Record(ctx, duration.Seconds(), metric.WithAttributes(
			attribute.String("usage.timeframe", string(timeframe)),
			attribute.String("usage.browser.phase", phase),
		))
	}
}
