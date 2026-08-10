package statequery

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"net/url"
	"os"
	"testing"

	"github.com/jackc/pgx/v5"
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
		RoleAdmin: withRole("k8s_role_admin", "test-role-admin"),
	}
}

func TestReconcileFixedRolesCreatesLeastPrivilegeRoles(t *testing.T) {
	adminURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if adminURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run the Postgres role test")
	}
	ctx := context.Background()
	pool, err := pgxpool.New(ctx, adminURL)
	if err != nil {
		t.Fatal(err)
	}
	defer pool.Close()

	const legacyTenant = "k8s_legacy_query_broker_tenant"
	if _, err := pool.Exec(ctx, "drop role if exists k8s_query_broker"); err != nil {
		t.Fatalf("drop existing query broker: %v", err)
	}
	if _, err := pool.Exec(ctx, "drop role if exists "+legacyTenant); err != nil {
		t.Fatalf("drop existing legacy tenant: %v", err)
	}
	defer func() {
		if _, err := pool.Exec(ctx, "drop role if exists "+legacyTenant); err != nil {
			t.Errorf("drop legacy tenant: %v", err)
		}
	}()
	if _, err := pool.Exec(ctx, "create role "+legacyTenant+" nologin"); err != nil {
		t.Fatalf("create legacy tenant: %v", err)
	}
	if _, err := pool.Exec(ctx, "create role k8s_query_broker login noinherit password 'legacy-broker'"); err != nil {
		t.Fatalf("create legacy query broker: %v", err)
	}
	if _, err := pool.Exec(ctx, "grant "+legacyTenant+" to k8s_query_broker"); err != nil {
		t.Fatalf("grant legacy tenant to query broker: %v", err)
	}

	if err := ReconcileFixedRoles(ctx, adminURL, fixedTestRoleURLs(t, adminURL)); err != nil {
		t.Fatalf("ReconcileFixedRoles: %v", err)
	}

	expected := map[string]struct {
		login, inherit, createRole bool
	}{
		"k8s_state_owner":    {false, true, false},
		"k8s_state_writer":   {true, true, false},
		"k8s_state_exporter": {true, true, false},
		"k8s_query_tenant":   {false, true, false},
		"k8s_query_admin":    {false, true, false},
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

	var brokerExists bool
	if err := pool.QueryRow(ctx, `select exists(select 1 from pg_roles where rolname = 'k8s_query_broker')`).Scan(&brokerExists); err != nil {
		t.Fatal(err)
	}
	if brokerExists {
		t.Fatal("legacy query broker still exists")
	}

	var legacyMembershipExists bool
	if err := pool.QueryRow(ctx, `
		select exists (
			select 1
			from pg_auth_members membership
			join pg_roles parent on parent.oid = membership.roleid
			join pg_roles member on member.oid = membership.member
			where parent.rolname = $1 and member.rolname = 'k8s_query_broker'
		)`, legacyTenant).Scan(&legacyMembershipExists); err != nil {
		t.Fatal(err)
	}
	if legacyMembershipExists {
		t.Fatal("legacy query broker membership remains")
	}

	var tenantAdminOption bool
	if err := pool.QueryRow(ctx, `
		select membership.admin_option
		from pg_auth_members membership
		join pg_roles parent on parent.oid = membership.roleid
		join pg_roles member on member.oid = membership.member
		where parent.rolname = 'k8s_query_tenant' and member.rolname = 'k8s_role_admin'
	`).Scan(&tenantAdminOption); err != nil {
		t.Fatalf("query role administrator tenant membership: %v", err)
	}
	if !tenantAdminOption {
		t.Fatal("role administrator lacks admin option for tenant role")
	}
}

func TestReconcileFixedRolesWithoutLegacyQueryBroker(t *testing.T) {
	adminURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if adminURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run the Postgres role test")
	}
	ctx := context.Background()
	pool, err := pgxpool.New(ctx, adminURL)
	if err != nil {
		t.Fatal(err)
	}
	defer pool.Close()

	if _, err := pool.Exec(ctx, "drop role if exists k8s_query_broker"); err != nil {
		t.Fatalf("drop existing query broker: %v", err)
	}
	if err := ReconcileFixedRoles(ctx, adminURL, fixedTestRoleURLs(t, adminURL)); err != nil {
		t.Fatalf("ReconcileFixedRoles without legacy broker: %v", err)
	}

	var tenantAdminOption bool
	if err := pool.QueryRow(ctx, `
		select membership.admin_option
		from pg_auth_members membership
		join pg_roles parent on parent.oid = membership.roleid
		join pg_roles member on member.oid = membership.member
		where parent.rolname = 'k8s_query_tenant' and member.rolname = 'k8s_role_admin'
	`).Scan(&tenantAdminOption); err != nil {
		t.Fatalf("query role administrator tenant membership: %v", err)
	}
	if !tenantAdminOption {
		t.Fatal("role administrator lacks admin option for tenant role")
	}
}

func reconcileTestTenantRole(t *testing.T, admin *pgxpool.Pool, tenant, password string) string {
	t.Helper()

	ctx := context.Background()
	role := TenantRoleName(tenant)
	identifier := pgx.Identifier{role}.Sanitize()
	t.Cleanup(func() {
		if _, err := admin.Exec(context.Background(), `select k8s_state.unregister_tenant_role($1)`, role); err != nil {
			t.Errorf("unregister tenant role %s: %v", role, err)
		}
		if _, err := admin.Exec(context.Background(), "drop role if exists "+identifier); err != nil {
			t.Errorf("drop tenant role %s: %v", role, err)
		}
	})
	if _, err := admin.Exec(ctx, "drop role if exists "+identifier); err != nil {
		t.Fatal(err)
	}

	var passwordClause string
	if err := admin.QueryRow(
		ctx,
		`select format(' password %L', $1::text)`,
		password,
	).Scan(&passwordClause); err != nil {
		t.Fatal(err)
	}
	for _, statement := range []string{
		"create role " + identifier + " login inherit nocreaterole" + passwordClause,
		"grant k8s_query_tenant to " + identifier,
	} {
		if _, err := admin.Exec(ctx, statement); err != nil {
			t.Fatal(err)
		}
	}

	fingerprint := sha256.Sum256([]byte(password))
	if _, err := admin.Exec(
		ctx,
		`select k8s_state.register_tenant_role($1, $2, $3)`,
		role,
		tenant,
		hex.EncodeToString(fingerprint[:]),
	); err != nil {
		t.Fatal(err)
	}
	return role
}
