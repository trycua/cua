package accountlookup

import (
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5/pgconn"
)

func TestStoreConstructionDoesNotRequireDatabase(t *testing.T) {
	store, err := NewStore(context.Background(), "postgres://test:test@127.0.0.1:1/test?connect_timeout=1")
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	if _, _, err := store.Resolve(context.Background(), "realm", "key", "user_test"); err == nil {
		t.Fatal("expected unavailable database error")
	}
}
func TestStoreConfigurationErrorsAreSanitized(t *testing.T) {
	for _, dsn := range []string{"", "://secret-value"} {
		_, err := NewStore(context.Background(), dsn)
		if err == nil || strings.Contains(err.Error(), "secret-value") {
			t.Fatalf("unexpected error: %v", err)
		}
		if dsn != "" {
			var parseErr *pgconn.ParseConfigError
			if errors.Unwrap(err) == nil || !errors.As(err, &parseErr) || !errors.Is(err, parseErr) {
				t.Fatal("configuration error must preserve its parser cause")
			}
		}
	}
}
