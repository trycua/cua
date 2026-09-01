package featureflagadmin

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"log/slog"

	"github.com/trycua/cloud/pkg/featureflags"
)

type slogAuditLogger struct {
	logger *slog.Logger
}

type auditTypedValue struct {
	ValueType featureflags.ValueType `json:"value_type"`
	Value     any                    `json:"value"`
	RawValue  string                 `json:"raw_value"`
}

func NewSlogAuditLogger(logger *slog.Logger) AuditLogger {
	if logger == nil {
		logger = slog.Default()
	}
	return &slogAuditLogger{logger: logger}
}

func (logger *slogAuditLogger) Log(ctx context.Context, event AuditEvent) {
	logger.logger.WarnContext(ctx, "feature flag mutation audited",
		"event", event.Event,
		"timestamp", event.Timestamp,
		"actor", event.Actor.Subject,
		"actor_email", event.Actor.Email,
		"principal_type", event.Actor.PrincipalType,
		"operation", event.Operation,
		"key", auditKey(event.Key),
		"path", auditPath(event.Path, event.Key),
		"traceId", event.Actor.TraceID,
		"ownership", event.Ownership,
		"old_value", auditValue(event.OldValue),
		"new_value", auditValue(event.NewValue),
		"expected_version", event.ExpectedVersion,
		"previous_version", event.PreviousVersion,
		"result_version", event.ResultVersion,
		"result", event.Result,
		"reason", event.Reason,
	)
}

func auditValue(value *featureflags.TypedValue) any {
	if value == nil {
		return nil
	}
	return auditTypedValue{ValueType: value.Type, Value: value.Value, RawValue: value.Raw}
}

const maxAuditIdentifierBytes = 128

func BoundedAuditKey(value string) string {
	return auditKey(value)
}

func BoundedAuditPath(path, key string) string {
	return auditPath(path, key)
}

func boundedAuditIdentifier(value string) string {
	if len(value) <= maxAuditIdentifierBytes {
		return value
	}
	sum := sha256.Sum256([]byte(value))
	prefix := value[:96]
	return fmt.Sprintf("%s... sha256:%s", prefix, hex.EncodeToString(sum[:8]))
}

func auditKey(value string) string {
	if value == "" || validateKey(value) == nil {
		return boundedAuditIdentifier(value)
	}
	return hashedAuditIdentifier(value)
}

func auditPath(path, key string) string {
	if key != "" && validateKey(key) != nil {
		return hashedAuditIdentifier(path)
	}
	return boundedAuditIdentifier(path)
}

func hashedAuditIdentifier(value string) string {
	sum := sha256.Sum256([]byte(value))
	return fmt.Sprintf("sha256:%s", hex.EncodeToString(sum[:16]))
}
