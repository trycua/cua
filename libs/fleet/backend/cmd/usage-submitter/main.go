// cyclops-usage-submitter turns completed reservation hours into Stripe
// meter events with a local ledger and an S3 archive written first.
//
// This build is dry-run only: SUBMIT_DRY_RUN must be "true" (the default).
// Design: docs/superpowers/specs/2026-09-16-cua-1158-stripe-usage-submitter-design.md
package main

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"strconv"
	"time"

	"cyclops-cs-backend/submitter"
	"cyclops-cs-backend/telemetry"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/metric"
)

func main() {
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stdout, nil)))
	if err := run(context.Background(), time.Now); err != nil {
		if errors.Is(err, submitter.ErrAlreadyRunning) {
			slog.Info("usage submitter skipped: another run holds the lock")
			return
		}
		slog.Error("usage submission failed", "error", err)
		os.Exit(1)
	}
}

func run(ctx context.Context, now func() time.Time) error {
	cfg, err := loadConfig()
	if err != nil {
		return err
	}
	shutdown, err := telemetry.Init(ctx, telemetry.Config{
		Endpoint:         envDefault("OTEL_EXPORTER_OTLP_ENDPOINT", "https://otel.cua.ai"),
		Protocol:         envDefault("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf"),
		ServiceName:      envDefault("OTEL_SERVICE_NAME", "cyclops-usage-submitter"),
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

	ctx, span := otel.Tracer("cyclops-cs-backend/submitter").Start(ctx, "usage_submitter.run")
	defer span.End()
	span.SetAttributes(
		attribute.String("submitter.cluster_id", cfg.submitter.ClusterID),
		attribute.String("submitter.cutover_hour", cfg.submitter.CutoverHour.Format(time.RFC3339)),
		attribute.Bool("submitter.dry_run", cfg.submitter.DryRun),
	)

	store, err := submitter.NewPostgresStore(ctx, cfg.databaseURL)
	if err != nil {
		span.SetStatus(codes.Error, "database initialization failed")
		return err
	}
	defer store.Close()
	archiver, err := submitter.NewS3Archiver(ctx)
	if err != nil {
		span.SetStatus(codes.Error, "archive initialization failed")
		return err
	}

	started := now()
	result, runErr := submitter.Runner{
		Config:    cfg.submitter,
		Store:     store,
		Archiver:  archiver,
		Customers: submitter.NoCustomers{},
		Now:       now,
	}.Run(ctx)

	meter := otel.Meter("cyclops-cs-backend/submitter")
	duration, _ := meter.Float64Histogram("cyclops.usage_submitter.run.duration", metric.WithDescription("Usage submitter run duration."), metric.WithUnit("s"))
	duration.Record(ctx, now().Sub(started).Seconds())
	if runErr != nil {
		if !errors.Is(runErr, submitter.ErrAlreadyRunning) {
			failures, _ := meter.Int64Counter("cyclops.usage_submitter.run.failures")
			failures.Add(ctx, 1)
			span.SetStatus(codes.Error, "usage submission failed")
		}
		return runErr
	}
	submissions, _ := meter.Int64Counter("cyclops.usage_submitter.submissions")
	submissions.Add(ctx, int64(result.Created), outcome("created"))
	submissions.Add(ctx, int64(result.Unchanged), outcome("unchanged"))
	submissions.Add(ctx, int64(result.Superseded), outcome("superseded"))
	submissions.Add(ctx, int64(result.DryRun), outcome("dry_run"))
	submissions.Add(ctx, int64(result.Unmapped), outcome("unmapped"))
	age, _ := meter.Float64Gauge("cyclops.usage_submitter.oldest_unsent_hour_age", metric.WithDescription("Age of the oldest eligible tenant-hour without a terminal submission."), metric.WithUnit("s"))
	if !result.OldestUnsentHour.IsZero() {
		age.Record(ctx, now().Sub(result.OldestUnsentHour).Seconds())
	} else {
		age.Record(ctx, 0)
	}
	span.SetAttributes(
		attribute.String("submitter.batch_id", result.BatchID.String()),
		attribute.Int("submitter.hours_considered", result.HoursConsidered),
		attribute.Int("submitter.tenant_hours", result.TenantHours),
		attribute.Int("submitter.created", result.Created),
		attribute.Int("submitter.unchanged", result.Unchanged),
		attribute.Int("submitter.resumed_batches", result.ResumedBatches),
	)
	slog.Info("usage submission complete",
		"batch_id", result.BatchID.String(),
		"dry_run", cfg.submitter.DryRun,
		"hours_considered", result.HoursConsidered,
		"tenant_hours", result.TenantHours,
		"created", result.Created,
		"unchanged", result.Unchanged,
		"superseded", result.Superseded,
		"dry_run_marked", result.DryRun,
		"resumed_batches", result.ResumedBatches)
	return nil
}

type config struct {
	databaseURL string
	submitter   submitter.Config
}

func loadConfig() (config, error) {
	cfg := config{
		databaseURL: os.Getenv("SUBMIT_DATABASE_URL"),
		submitter: submitter.Config{
			ClusterID:       envDefault("SUBMIT_CLUSTER_ID", "kopf-k3s"),
			SettlementLag:   2 * time.Hour,
			Lookback:        34 * 24 * time.Hour,
			DryRun:          true,
			ArchiveBucket:   os.Getenv("SUBMIT_ARCHIVE_BUCKET"),
			ArchivePrefix:   os.Getenv("SUBMIT_ARCHIVE_PREFIX"),
			ExporterVersion: envDefault("SUBMIT_EXPORTER_VERSION", "dev"),
		},
	}
	if cfg.databaseURL == "" {
		return config{}, errors.New("SUBMIT_DATABASE_URL is required")
	}
	if cfg.submitter.ArchiveBucket == "" {
		return config{}, errors.New("SUBMIT_ARCHIVE_BUCKET is required")
	}
	rawCutover := os.Getenv("SUBMIT_CUTOVER_HOUR")
	if rawCutover == "" {
		return config{}, errors.New("SUBMIT_CUTOVER_HOUR is required and has no default: it is the manual-to-automated billing boundary")
	}
	cutover, err := time.Parse(time.RFC3339, rawCutover)
	if err != nil {
		return config{}, fmt.Errorf("SUBMIT_CUTOVER_HOUR must be an exact UTC RFC3339 hour: %w", err)
	}
	cfg.submitter.CutoverHour = cutover.UTC()
	if !cfg.submitter.CutoverHour.Equal(cfg.submitter.CutoverHour.Truncate(time.Hour)) {
		return config{}, errors.New("SUBMIT_CUTOVER_HOUR must be an exact UTC RFC3339 hour")
	}
	if cfg.submitter.SettlementLag, err = durationEnv("SUBMIT_SETTLEMENT_LAG", cfg.submitter.SettlementLag); err != nil {
		return config{}, err
	}
	if cfg.submitter.Lookback, err = durationEnv("SUBMIT_LOOKBACK", cfg.submitter.Lookback); err != nil {
		return config{}, err
	}
	if raw := os.Getenv("SUBMIT_DRY_RUN"); raw != "" {
		dryRun, err := strconv.ParseBool(raw)
		if err != nil {
			return config{}, fmt.Errorf("SUBMIT_DRY_RUN must be a boolean: %w", err)
		}
		cfg.submitter.DryRun = dryRun
	}
	if !cfg.submitter.DryRun {
		return config{}, errors.New("SUBMIT_DRY_RUN=false is not supported by this build; live submission is not implemented")
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

func outcome(value string) metric.AddOption {
	return metric.WithAttributes(attribute.String("outcome", value))
}
