package featureflagadmin

import (
	"bytes"
	"context"
	"encoding/json"
	"log/slog"
	"strings"
	"testing"
	"time"

	"github.com/trycua/cloud/pkg/featureflags"
)

func TestSlogAuditLoggerEmitsStableJSONSchema(t *testing.T) {
	var output bytes.Buffer
	logger := slog.New(slog.NewJSONHandler(&output, nil))
	timestamp := time.Date(2026, 8, 13, 14, 15, 16, 0, time.UTC)
	oldValue := featureflags.TypedValue{Type: featureflags.ValueBoolean, Value: false, Raw: "false"}
	newValue := featureflags.TypedValue{Type: featureflags.ValueJSON, Value: map[string]any{"enabled": true}, Raw: `{"enabled":true}`}

	NewSlogAuditLogger(logger).Log(context.Background(), AuditEvent{
		Event:     "feature_flag_admin",
		Timestamp: timestamp,
		Actor: Actor{
			Subject:       "admin-1",
			Email:         "admin@example.com",
			PrincipalType: "user",
			TraceID:       "trace-123",
		},
		Operation:       "update",
		Key:             "example-flag",
		Path:            Prefix + "example-flag",
		Ownership:       OwnershipTerraform,
		OldValue:        &oldValue,
		NewValue:        &newValue,
		ExpectedVersion: 4,
		PreviousVersion: 4,
		ResultVersion:   5,
		Result:          "success",
		Reason:          "",
	})

	var record map[string]any
	if err := json.Unmarshal(output.Bytes(), &record); err != nil {
		t.Fatalf("unmarshal audit log: %v; output=%s", err, output.String())
	}
	wants := map[string]any{
		"event": "feature_flag_admin", "actor": "admin-1", "actor_email": "admin@example.com",
		"principal_type": "user", "operation": "update", "key": "example-flag",
		"path": Prefix + "example-flag", "traceId": "trace-123", "ownership": "terraform",
		"expected_version": float64(4), "previous_version": float64(4), "result_version": float64(5),
		"result": "success", "reason": "", "timestamp": timestamp.Format(time.RFC3339),
	}
	for key, want := range wants {
		if got := record[key]; got != want {
			t.Errorf("%s = %#v, want %#v", key, got, want)
		}
	}
	assertTypedAuditValue(t, record["old_value"], "boolean", false, "false")
	assertTypedAuditValue(t, record["new_value"], "json", map[string]any{"enabled": true}, `{"enabled":true}`)
}

func TestSlogAuditLoggerBoundsUnvalidatedIdentifiers(t *testing.T) {
	var output bytes.Buffer
	logger := slog.New(slog.NewJSONHandler(&output, nil))
	invalidKey := "Sensitive/Secret" + strings.Repeat("-value", 100)
	NewSlogAuditLogger(logger).Log(context.Background(), AuditEvent{
		Event: "feature_flag_admin", Key: invalidKey, Path: Prefix + invalidKey, Result: "rejected", Reason: "invalid_key",
	})
	if strings.Contains(output.String(), invalidKey) || strings.Contains(output.String(), "Sensitive/Secret") {
		t.Fatalf("audit contains unvalidated invalid key: %s", output.String())
	}
	var record map[string]any
	if err := json.Unmarshal(output.Bytes(), &record); err != nil {
		t.Fatal(err)
	}
	for _, field := range []string{"key", "path"} {
		value, _ := record[field].(string)
		if len(value) > 160 || !strings.Contains(value, "sha256:") {
			t.Errorf("%s = %q, want bounded hash marker", field, value)
		}
	}
}

func assertTypedAuditValue(t *testing.T, raw any, wantType string, wantValue any, wantRaw string) {
	t.Helper()
	value, ok := raw.(map[string]any)
	if !ok {
		t.Fatalf("typed value = %#v, want object", raw)
	}
	if value["value_type"] != wantType || value["raw_value"] != wantRaw {
		t.Fatalf("typed value metadata = %#v", value)
	}
	wantJSON, _ := json.Marshal(wantValue)
	gotJSON, _ := json.Marshal(value["value"])
	if !bytes.Equal(gotJSON, wantJSON) {
		t.Fatalf("typed value = %s, want %s", gotJSON, wantJSON)
	}
}
