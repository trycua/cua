package main

import (
	"context"
	"log/slog"
	"os"

	"cyclops-cs-backend/database"
)

func required(name string) string {
	value := os.Getenv(name)
	if value == "" {
		slog.Error("required migration environment variable is unset", "name", name)
		os.Exit(2)
	}
	return value
}

func logMigrationFailure(err error) {
	classification := database.ClassifyError(err)
	slog.Error("database migration failed",
		"class", classification.Class,
		"retryable", classification.Retryable)
}

func main() {
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stdout, nil)))

	config := database.Config{
		MigrationURL: required("MIGRATION_DATABASE_URL"),
		Credentials: database.CredentialURLs{
			Application: required("DATABASE_URL"),
			Writer:      required("STATE_WRITER_DATABASE_URL"),
			Exporter:    required("STATE_EXPORTER_DATABASE_URL"),
			RoleAdmin:   required("STATE_ROLE_ADMIN_DATABASE_URL"),
			Metabase:    required("METABASE_DATABASE_URL"),
			Usage:       required("USAGE_DATABASE_URL"),
			Meter:       required("METER_DATABASE_URL"),
		},
	}
	if err := database.Run(context.Background(), config); err != nil {
		logMigrationFailure(err)
		os.Exit(1)
	}
	slog.Info("database migrations complete")
}
