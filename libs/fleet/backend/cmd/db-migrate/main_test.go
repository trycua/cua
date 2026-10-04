package main

import (
	"bytes"
	"errors"
	"log/slog"
	"strings"
	"testing"
)

func TestLogMigrationFailureDoesNotRenderCause(t *testing.T) {
	const secret = "postgres://user:secret-password@db.internal/cyclops"
	var logged bytes.Buffer
	restore := slog.Default()
	slog.SetDefault(slog.New(slog.NewJSONHandler(&logged, nil)))
	t.Cleanup(func() { slog.SetDefault(restore) })

	logMigrationFailure(errors.New(secret))

	if strings.Contains(logged.String(), secret) || strings.Contains(logged.String(), "secret-password") {
		t.Fatalf("migration failure log leaked cause:\n%s", logged.String())
	}
	if !strings.Contains(logged.String(), "database migration failed") {
		t.Fatalf("migration failure log omitted event message:\n%s", logged.String())
	}
	if !strings.Contains(logged.String(), `"class":"internal"`) {
		t.Fatalf("migration failure log omitted safe class:\n%s", logged.String())
	}
}
