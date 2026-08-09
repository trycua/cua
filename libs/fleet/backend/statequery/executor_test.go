package statequery

import (
	"context"
	"os"
	"testing"

	"github.com/jackc/pgx/v5/pgxpool"
)

func TestTenantRoleNameIsStableAndOpaque(t *testing.T) {
	got := TenantRoleName("user-alice")
	if got != "k8s_tenant_0e7b8c3e3b7f94ed81538a568a6408c6" {
		t.Fatalf("TenantRoleName = %q", got)
	}
	if got == TenantRoleName("user-bob") {
		t.Fatal("different tenants received the same role")
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
	defer admin.Close()
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

	executor, err := NewExecutor(ctx, urls.Query, urls.RoleAdmin)
	if err != nil {
		t.Fatal(err)
	}
	defer executor.Close()
	validated, err := Validate(`select name from k8s_api.current_resources where cluster_id = 'rls-test' and resource = 'namespaces' order by name`, DefaultLimits())
	if err != nil {
		t.Fatal(err)
	}
	result, err := executor.Execute(ctx, "user-alice", false, validated)
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Rows) != 1 || result.Rows[0][0] != "alice-ns" {
		t.Fatalf("rows = %#v, want only alice-ns", result.Rows)
	}
}
