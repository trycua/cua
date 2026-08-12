package statequery

import (
	"context"
	"os"
	"testing"

	"github.com/jackc/pgx/v5/pgxpool"
)

func TestMigrateCreatesStateSchemaAndRLS(t *testing.T) {
	url := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if url == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run the Postgres schema test")
	}
	ctx := context.Background()
	if err := ReconcileFixedRoles(ctx, url, fixedTestRoleURLs(t, url)); err != nil {
		t.Fatalf("ReconcileFixedRoles: %v", err)
	}
	if err := Migrate(ctx, url); err != nil {
		t.Fatalf("Migrate: %v", err)
	}
	if err := Migrate(ctx, url); err != nil {
		t.Fatalf("idempotent Migrate: %v", err)
	}

	pool, err := pgxpool.New(ctx, url)
	if err != nil {
		t.Fatal(err)
	}
	defer pool.Close()

	for _, relation := range []string{
		"k8s_state.resource_state",
		"k8s_state.watch_checkpoint",
		"k8s_state.resource_event_outbox",
		"k8s_state.resource_schema",
		"k8s_state.query_tenant_role",
		"k8s_api.current_resources",
	} {
		var exists bool
		if err := pool.QueryRow(ctx, `select to_regclass($1) is not null`, relation).Scan(&exists); err != nil {
			t.Fatal(err)
		}
		if !exists {
			t.Errorf("relation %s does not exist", relation)
		}
	}

	for _, column := range []string{"role_name", "capsule_tenant", "credential_fingerprint", "created_at"} {
		var exists bool
		err := pool.QueryRow(ctx, `
			select exists (
				select 1
				from information_schema.columns
				where table_schema = 'k8s_state'
				  and table_name = 'query_tenant_role'
				  and column_name = $1
			)`, column).Scan(&exists)
		if err != nil || !exists {
			t.Fatalf("query_tenant_role column %s exists=%t err=%v", column, exists, err)
		}
	}

	for _, function := range []string{
		"k8s_state.register_tenant_role(name,text,text)",
		"k8s_state.unregister_tenant_role(name)",
	} {
		var exists bool
		if err := pool.QueryRow(ctx, "select to_regprocedure($1) is not null", function).Scan(&exists); err != nil || !exists {
			t.Fatalf("function %s exists=%t err=%v", function, exists, err)
		}
	}

	var forced bool
	if err := pool.QueryRow(ctx, `select relforcerowsecurity from pg_class where oid = 'k8s_state.resource_state'::regclass`).Scan(&forced); err != nil {
		t.Fatal(err)
	}
	if !forced {
		t.Fatal("resource_state does not FORCE ROW LEVEL SECURITY")
	}
}
