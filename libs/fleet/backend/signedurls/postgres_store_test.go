package signedurls

import (
	"context"
	"errors"
	"os"
	"testing"
	"time"

	"cyclops-cs-backend/database"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

func TestSignedServiceURLDatabaseErrorClassifiesUnavailablePostgresStates(t *testing.T) {
	for _, code := range []string{"42P01", "08006", "28000", "53300", "57P01", "57P02", "57P03"} {
		t.Run(code, func(t *testing.T) {
			err := signedServiceURLDatabaseError("query", &pgconn.PgError{Code: code})
			if !errors.Is(err, ErrUnavailable) {
				t.Fatalf("error %v does not classify SQLSTATE %s as unavailable", err, code)
			}
		})
	}
	if errors.Is(signedServiceURLDatabaseError("query", &pgconn.PgError{Code: "23505"}), ErrUnavailable) {
		t.Fatal("integrity violation classified as unavailable")
	}
}

func TestPostgresStoreRoundTrip(t *testing.T) {
	databaseURL := os.Getenv("CYCLOPS_TEST_RUNTIME_DATABASE_URL")
	if databaseURL == "" {
		t.Skip("set CYCLOPS_TEST_RUNTIME_DATABASE_URL to run the Postgres store test")
	}
	if err := database.RequireVersion(context.Background(), databaseURL, 11); err != nil {
		t.Fatalf("database must be migrated to version 11: %v", err)
	}
	store, err := NewPostgresStore(context.Background(), databaseURL)
	if err != nil {
		t.Fatalf("NewPostgresStore() error = %v", err)
	}
	t.Cleanup(store.Close)

	record := signedURLFixture()
	record.ID = uuid.New()
	record.ClaimName = "claim-roundtrip-" + record.ID.String()
	record.SandboxName = "sandbox-roundtrip"
	record.LogicalService = "mcp"
	record.CreatorSub = "user-roundtrip"
	// The application role deliberately has no DELETE grant on
	// signed_service_urls, so cleanup runs as the migration owner.
	t.Cleanup(func() {
		maintenanceURL := os.Getenv("CYCLOPS_TEST_MIGRATION_DATABASE_URL")
		if maintenanceURL == "" {
			return
		}
		connection, err := pgx.Connect(context.Background(), maintenanceURL)
		if err != nil {
			t.Errorf("connect for signed service URL cleanup: %v", err)
			return
		}
		defer connection.Close(context.Background())
		if _, err := connection.Exec(context.Background(), "DELETE FROM signed_service_urls WHERE id = $1", record.ID); err != nil {
			t.Errorf("cleanup signed service URL %s: %v", record.ID, err)
		}
	})
	if err := store.Create(context.Background(), record); err != nil {
		t.Fatalf("Create() error = %v", err)
	}

	listed, err := store.List(context.Background(), record.Namespace, record.ClaimName)
	if err != nil || len(listed) == 0 {
		t.Fatalf("List() = %#v, %v", listed, err)
	}
	got, err := store.Get(context.Background(), record.ID)
	if err != nil || got.ID != record.ID {
		t.Fatalf("Get() = %#v, %v", got, err)
	}

	revokedAt := time.Now().UTC().Add(time.Second)
	first, err := store.Revoke(context.Background(), record.Namespace, record.ID, revokedAt)
	if err != nil || first.RevokedAt == nil {
		t.Fatalf("first Revoke() = %#v, %v", first, err)
	}
	second, err := store.Revoke(context.Background(), record.Namespace, record.ID, revokedAt.Add(time.Minute))
	if err != nil || !second.RevokedAt.Equal(*first.RevokedAt) {
		t.Fatalf("second Revoke() = %#v, %v", second, err)
	}
}
