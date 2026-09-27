package main

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"time"

	"cyclops-cs-backend/metering"
	"cyclops-cs-backend/telemetry"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/metric"
)

func main() {
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stdout, nil)))
	if err := run(context.Background(), time.Now); err != nil {
		slog.Error("hourly reservation metering failed", "error", err)
		os.Exit(1)
	}
}

func run(ctx context.Context, now func() time.Time) error {
	cfg, err := loadConfig(now())
	if err != nil {
		return err
	}
	shutdown, err := telemetry.Init(ctx, telemetry.Config{
		Endpoint:         envDefault("OTEL_EXPORTER_OTLP_ENDPOINT", "https://otel.cua.ai"),
		Protocol:         envDefault("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf"),
		ServiceName:      envDefault("OTEL_SERVICE_NAME", "cyclops-usage-meter"),
		ServiceNamespace: envDefault("OTEL_SERVICE_NAMESPACE", "cyclops-cs"),
		Environment:      envDefault("OTEL_ENVIRONMENT", "production"),
		ResourceAttrs:    os.Getenv("OTEL_RESOURCE_ATTRIBUTES"),
	})
	if err != nil {
		return fmt.Errorf("initialize telemetry: %w", err)
	}
	defer func() {
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		if err := shutdown(shutdownCtx); err != nil {
			slog.Warn("telemetry shutdown failed")
		}
	}()

	ctx, span := otel.Tracer("cyclops-cs-backend/metering").Start(ctx, "usage_meter.collect_hour")
	defer span.End()
	span.SetAttributes(
		attribute.String("meter.cluster_id", cfg.clusterID),
		attribute.String("meter.hour_start", cfg.hourStart.Format(time.RFC3339)),
		attribute.Int64("meter.minimum_coverage_seconds", int64(cfg.minimumCoverage.Seconds())),
	)

	prometheus, err := metering.NewPrometheusClient(cfg.prometheusURL, nil)
	if err != nil {
		span.SetStatus(codes.Error, "invalid Prometheus configuration")
		return err
	}
	store, err := metering.NewPostgresStore(ctx, cfg.databaseURL)
	if err != nil {
		span.SetStatus(codes.Error, "database initialization failed")
		return err
	}
	defer store.Close()

	started := time.Now()
	result, err := (metering.Collector{
		Prometheus:        prometheus,
		Tenants:           store,
		Writer:            store,
		ClusterID:         cfg.clusterID,
		Step:              cfg.step,
		MaxSampleValidity: cfg.maxSampleValidity,
		MinimumCoverage:   cfg.minimumCoverage,
	}).CollectHour(ctx, cfg.hourStart)
	meter := otel.Meter("cyclops-cs-backend/metering")
	duration, _ := meter.Float64Histogram("cyclops.usage_meter.collect.duration", meteringMetricOptions("Hourly reservation collection duration.")...)
	duration.Record(ctx, time.Since(started).Seconds())
	if err != nil {
		failures, _ := meter.Int64Counter("cyclops.usage_meter.collect.failures")
		failures.Add(ctx, 1)
		span.SetStatus(codes.Error, "hourly reservation collection failed")
		return err
	}
	facts, _ := meter.Int64Counter("cyclops.usage_meter.facts")
	facts.Add(ctx, int64(result.Inserted), attributeMetric("outcome", "inserted"))
	facts.Add(ctx, int64(result.Unchanged), attributeMetric("outcome", "unchanged"))
	facts.Add(ctx, int64(result.Unattributed), attributeMetric("outcome", "unattributed"))
	coverage, _ := meter.Float64Histogram("cyclops.usage_meter.coverage", meteringMetricOptions("KSM source coverage for a materialized hour.")...)
	coverage.Record(ctx, result.Coverage.Seconds())
	span.SetAttributes(
		attribute.Int("meter.sandboxes_discovered", result.Discovered),
		attribute.Int("meter.facts_inserted", result.Inserted),
		attribute.Int("meter.facts_unchanged", result.Unchanged),
		attribute.Int("meter.sandboxes_unattributed", result.Unattributed),
		attribute.Int64("meter.coverage_seconds", int64(result.Coverage.Seconds())),
	)
	slog.Info("hourly reservation metering complete",
		"hour_start", cfg.hourStart.Format(time.RFC3339),
		"discovered", result.Discovered,
		"inserted", result.Inserted,
		"unchanged", result.Unchanged,
		"unattributed", result.Unattributed,
		"coverage_seconds", int64(result.Coverage.Seconds()))
	return nil
}

type config struct {
	databaseURL       string
	prometheusURL     string
	clusterID         string
	hourStart         time.Time
	step              time.Duration
	maxSampleValidity time.Duration
	minimumCoverage   time.Duration
}

func loadConfig(now time.Time) (config, error) {
	cfg := config{
		databaseURL:       os.Getenv("METER_DATABASE_URL"),
		prometheusURL:     envDefault("PROMETHEUS_URL", "http://prometheus.cyclops-cs.svc.cluster.local"),
		clusterID:         envDefault("METER_CLUSTER_ID", "kopf-k3s"),
		step:              15 * time.Second,
		maxSampleValidity: time.Minute,
		minimumCoverage:   3570 * time.Second,
	}
	if cfg.databaseURL == "" {
		return config{}, fmt.Errorf("METER_DATABASE_URL is required")
	}
	var err error
	if cfg.step, err = durationEnv("METER_SCRAPE_INTERVAL", cfg.step); err != nil {
		return config{}, err
	}
	if cfg.maxSampleValidity, err = durationEnv("METER_MAX_SAMPLE_VALIDITY", cfg.maxSampleValidity); err != nil {
		return config{}, err
	}
	if cfg.minimumCoverage, err = durationEnv("METER_MINIMUM_COVERAGE", cfg.minimumCoverage); err != nil {
		return config{}, err
	}
	cfg.hourStart = now.UTC().Truncate(time.Hour).Add(-time.Hour)
	if rawHour := os.Getenv("METER_HOUR_START"); rawHour != "" {
		cfg.hourStart, err = time.Parse(time.RFC3339, rawHour)
		if err != nil {
			return config{}, fmt.Errorf("METER_HOUR_START must be an exact UTC RFC3339 hour: %w", err)
		}
		if !cfg.hourStart.Equal(cfg.hourStart.UTC().Truncate(time.Hour)) {
			return config{}, fmt.Errorf("METER_HOUR_START must be an exact UTC RFC3339 hour")
		}
	}
	return cfg, nil
}

func durationEnv(name string, fallback time.Duration) (time.Duration, error) {
	raw := os.Getenv(name)
	if raw == "" {
		return fallback, nil
	}
	value, err := time.ParseDuration(raw)
	if err != nil {
		return 0, fmt.Errorf("%s must be a positive duration: %w", name, err)
	}
	if value <= 0 {
		return 0, fmt.Errorf("%s must be a positive duration", name)
	}
	return value, nil
}

func envDefault(name, fallback string) string {
	if value := os.Getenv(name); value != "" {
		return value
	}
	return fallback
}

func meteringMetricOptions(description string) []metric.Float64HistogramOption {
	return []metric.Float64HistogramOption{metric.WithDescription(description), metric.WithUnit("s")}
}

func attributeMetric(key, value string) metric.AddOption {
	return metric.WithAttributes(attribute.String(key, value))
}
