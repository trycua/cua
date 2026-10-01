package usage

import (
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
)

func usageTracer() trace.Tracer {
	return otel.Tracer("cyclops-cs-backend/usage")
}

func markUsageSpanError(span trace.Span, description string) {
	span.SetStatus(codes.Error, description)
}
