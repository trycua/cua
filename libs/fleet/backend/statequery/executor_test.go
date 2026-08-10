package statequery

import (
	"context"
	"os"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
)

type collectingResultWriter struct {
	fields []pgconn.FieldDescription
	rows   [][]any
}

func (writer *collectingResultWriter) WriteFieldDescriptions(fields []pgconn.FieldDescription) error {
	writer.fields = fields
	return nil
}

func (writer *collectingResultWriter) WriteRow(row []any) error {
	writer.rows = append(writer.rows, row)
	return nil
}

func TestTenantRoleNameIsStableAndOpaque(t *testing.T) {
	got := TenantRoleName("user-alice")
	if got != "k8s_tenant_0e7b8c3e3b7f94ed81538a568a6408c6" {
		t.Fatalf("TenantRoleName = %q", got)
	}
	if got == TenantRoleName("user-bob") {
		t.Fatal("different tenants received the same role")
	}
}

func TestExecutorBuildsDirectTenantConnectionConfig(t *testing.T) {
	executor, err := NewExecutor(
		"postgres://db.example/cyclops?sslmode=require",
		"tenant-password",
	)
	if err != nil {
		t.Fatal(err)
	}
	config := executor.connectionConfig("user-alice")
	if config.User != TenantRoleName("user-alice") {
		t.Fatalf("User = %q", config.User)
	}
	if config.Password != "tenant-password" {
		t.Fatalf("Password = %q", config.Password)
	}
}

func TestNewExecutorRequiresDSNAndPassword(t *testing.T) {
	if _, err := NewExecutor("", "password"); err == nil {
		t.Fatal("accepted empty query DSN")
	}
	if _, err := NewExecutor("postgres://db/cyclops", ""); err == nil {
		t.Fatal("accepted empty tenant password")
	}
}

func TestExecutorListsOnlyTenantOwnedNamespaces(t *testing.T) {
	adminURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if adminURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run the PostgreSQL RLS test")
	}
	ctx := context.Background()
	urls := fixedTestRoleURLs(t, adminURL)
	if err := ReconcileFixedRoles(ctx, adminURL, urls); err != nil {
		t.Fatal(err)
	}
	if err := Migrate(ctx, adminURL); err != nil {
		t.Fatal(err)
	}
	admin, err := pgxpool.New(ctx, adminURL)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(admin.Close)
	_, err = admin.Exec(ctx, `
		delete from k8s_state.resource_state where cluster_id = 'rls-test';
		insert into k8s_state.resource_state
		(cluster_id, api_group, resource, namespace, name, capsule_tenant, schema_hash, watch_epoch, observed_sequence, labels, object)
		values
		('rls-test', '', 'namespaces', '', 'alice-ns', 'user-alice', 's', 1, 1, '{}', '{"kind":"Namespace"}'),
		('rls-test', '', 'namespaces', '', 'bob-ns', 'user-bob', 's', 1, 2, '{}', '{"kind":"Namespace"}'),
		('rls-test', '', 'nodes', '', 'node-a', 'user-alice', 's', 1, 3, '{}', '{"kind":"Node"}'),
		('rls-test', '', 'pods', 'alice-ns', 'pod-a', 'user-alice', 's', 1, 4, '{}', '{"kind":"Pod"}')
	`)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_, _ = admin.Exec(context.Background(), `delete from k8s_state.resource_state where cluster_id = 'rls-test'`)
	})

	reconcileTestTenantRole(t, admin, "user-alice", "test-tenant-password")

	executor, err := NewExecutor(adminURL, "test-tenant-password")
	if err != nil {
		t.Fatal(err)
	}
	writer := &collectingResultWriter{}
	err = executor.Execute(ctx, "user-alice", `select name from k8s_api.current_resources where cluster_id = 'rls-test' and resource = 'namespaces' order by name`, writer)
	if err != nil {
		t.Fatal(err)
	}
	if len(writer.rows) != 1 || writer.rows[0][0] != "alice-ns" {
		t.Fatalf("rows = %#v, want only alice-ns", writer.rows)
	}
}

func TestDirectTenantLoginCannotEscapeTenantIsolation(t *testing.T) {
	adminURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if adminURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run the PostgreSQL direct-login isolation test")
	}
	ctx := context.Background()
	if err := ReconcileFixedRoles(ctx, adminURL, fixedTestRoleURLs(t, adminURL)); err != nil {
		t.Fatal(err)
	}
	if err := Migrate(ctx, adminURL); err != nil {
		t.Fatal(err)
	}
	admin, err := pgxpool.New(ctx, adminURL)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(admin.Close)

	const (
		password = "test-tenant-password"
		alice    = "user-alice"
		bob      = "user-bob"
		cluster  = "direct-login-isolation-test"
	)
	aliceRole := reconcileTestTenantRole(t, admin, alice, password)
	bobRole := reconcileTestTenantRole(t, admin, bob, password)
	if _, err := admin.Exec(ctx, `delete from k8s_state.resource_state where cluster_id = $1`, cluster); err != nil {
		t.Fatal(err)
	}
	if _, err := admin.Exec(ctx, `
		insert into k8s_state.resource_state
		(cluster_id, api_group, resource, namespace, name, capsule_tenant, schema_hash, watch_epoch, observed_sequence, labels, object)
		values
		($1, '', 'namespaces', '', 'alice-ns', $2, 's', 1, 1, '{}', '{"kind":"Namespace"}'),
		($1, '', 'namespaces', '', 'bob-ns', $3, 's', 1, 2, '{}', '{"kind":"Namespace"}')
	`, cluster, alice, bob); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_, _ = admin.Exec(context.Background(), `delete from k8s_state.resource_state where cluster_id = $1`, cluster)
		_, _ = admin.Exec(context.Background(), `drop table if exists tenant_escape`)
	})

	aliceConfig, err := pgx.ParseConfig(adminURL)
	if err != nil {
		t.Fatal(err)
	}
	aliceConfig.User = aliceRole
	aliceConfig.Password = password
	aliceConn, err := pgx.ConnectConfig(ctx, aliceConfig)
	if err != nil {
		t.Fatal(err)
	}
	defer aliceConn.Close(ctx)

	if _, err = aliceConn.Exec(ctx, "set role "+pgx.Identifier{bobRole}.Sanitize()); err == nil {
		t.Fatal("alice switched to bob's tenant role")
	}

	rows, err := aliceConn.Query(ctx, `
		select name
		from k8s_api.current_resources
		where cluster_id = $1 and resource = 'namespaces'
		order by name
	`, cluster)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	var names []string
	for rows.Next() {
		var name string
		if err := rows.Scan(&name); err != nil {
			t.Fatal(err)
		}
		names = append(names, name)
	}
	if err := rows.Err(); err != nil {
		t.Fatal(err)
	}
	if len(names) != 1 || names[0] != "alice-ns" {
		t.Fatalf("rows = %q, want only alice-ns", names)
	}

	if _, err = aliceConn.Exec(ctx, `create table tenant_escape(id integer)`); err == nil {
		t.Fatal("tenant role created a table")
	}
}
