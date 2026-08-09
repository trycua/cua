package statequery

import (
	"context"
	"net/url"
	"os"
	"testing"

	"github.com/jackc/pgx/v5/pgxpool"
)

func fixedTestRoleURLs(t *testing.T, adminURL string) FixedRoleURLs {
	t.Helper()
	parsed, err := url.Parse(adminURL)
	if err != nil {
		t.Fatal(err)
	}
	withRole := func(role, password string) string {
		copy := *parsed
		copy.User = url.UserPassword(role, password)
		return copy.String()
	}
	return FixedRoleURLs{
		Writer:    withRole("k8s_state_writer", "test-writer"),
		Exporter:  withRole("k8s_state_exporter", "test-exporter"),
		Query:     withRole("k8s_query_broker", "test-query"),
		RoleAdmin: withRole("k8s_role_admin", "test-role-admin"),
	}
}

func TestReconcileFixedRolesCreatesLeastPrivilegeRoles(t *testing.T) {
	adminURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if adminURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run the Postgres role test")
	}
	ctx := context.Background()
	if err := ReconcileFixedRoles(ctx, adminURL, fixedTestRoleURLs(t, adminURL)); err != nil {
		t.Fatalf("ReconcileFixedRoles: %v", err)
	}

	pool, err := pgxpool.New(ctx, adminURL)
	if err != nil {
		t.Fatal(err)
	}
	defer pool.Close()

	expected := map[string]struct {
		login, inherit, createRole bool
	}{
		"k8s_state_owner":    {false, true, false},
		"k8s_state_writer":   {true, true, false},
		"k8s_state_exporter": {true, true, false},
		"k8s_query_tenant":   {false, true, false},
		"k8s_query_admin":    {false, true, false},
		"k8s_query_broker":   {true, false, false},
		"k8s_role_admin":     {true, false, true},
	}
	for role, want := range expected {
		var login, inherit, createRole bool
		err := pool.QueryRow(ctx, `select rolcanlogin, rolinherit, rolcreaterole from pg_roles where rolname = $1`, role).Scan(&login, &inherit, &createRole)
		if err != nil {
			t.Fatalf("query role %s: %v", role, err)
		}
		if login != want.login || inherit != want.inherit || createRole != want.createRole {
			t.Errorf("role %s flags = login:%t inherit:%t createrole:%t, want %+v", role, login, inherit, createRole, want)
		}
	}
}
