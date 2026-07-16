package telemetry

import (
	"context"
	"fmt"
	"strings"
	"sync"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracehttp"
	"go.opentelemetry.io/otel/propagation"
	"go.opentelemetry.io/otel/sdk/resource"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	semconv "go.opentelemetry.io/otel/semconv/v1.26.0"
)

type Config struct {
	Endpoint         string
	Protocol         string
	ServiceName      string
	ServiceNamespace string
	Environment      string
	ResourceAttrs    string
}

func Init(ctx context.Context, cfg Config) (func(context.Context) error, error) {
	if cfg.Endpoint == "" {
		return func(context.Context) error { return nil }, nil
	}
	if cfg.Protocol != "" && cfg.Protocol != "http/protobuf" {
		return nil, fmt.Errorf("unsupported OTEL protocol %q", cfg.Protocol)
	}

	res, err := resource.New(
		ctx,
		resource.WithAttributes(buildResourceAttributes(cfg)...),
	)
	if err != nil {
		return nil, fmt.Errorf("build resource: %w", err)
	}

	exporter, err := otlptracehttp.New(ctx, otlptracehttp.WithEndpointURL(cfg.Endpoint))
	if err != nil {
		return nil, fmt.Errorf("create otlp http trace exporter: %w", err)
	}

	provider := sdktrace.NewTracerProvider(
		sdktrace.WithBatcher(exporter),
		sdktrace.WithResource(res),
	)
	otel.SetTracerProvider(provider)
	otel.SetTextMapPropagator(propagation.NewCompositeTextMapPropagator(
		propagation.TraceContext{},
		propagation.Baggage{},
	))

	var once sync.Once
	return func(ctx context.Context) error {
		var shutdownErr error
		once.Do(func() {
			shutdownErr = provider.Shutdown(ctx)
		})
		return shutdownErr
	}, nil
}

func buildResourceAttributes(cfg Config) []attribute.KeyValue {
	attrs := []attribute.KeyValue{
		semconv.ServiceName(cfg.ServiceName),
		semconv.ServiceNamespace(cfg.ServiceNamespace),
		attribute.String("deployment.environment", cfg.Environment),
	}

	for _, part := range strings.Split(cfg.ResourceAttrs, ",") {
		key, value, ok := strings.Cut(strings.TrimSpace(part), "=")
		if !ok || key == "" || value == "" {
			continue
		}
		attrs = append(attrs, attribute.String(key, value))
	}
	return attrs
}
