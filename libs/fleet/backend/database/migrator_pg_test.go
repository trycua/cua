package database

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"net/url"
	"os"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
)

const migratorIntegrationOptIn = "CYCLOPS_TEST_DATABASE_MIGRATOR_ISOLATED_CLUSTER"

const tenantCredentialFingerprintLookup = `select credential_fingerprint from k8s_state.query_tenant_role where role_name = $1`

var staticMigrationRoles = []string{
	"cyclops_app",
	"k8s_state_owner",
	"k8s_state_writer",
	"k8s_state_exporter",
	"k8s_query_tenant",
	"k8s_query_admin",
	"k8s_role_admin",
	"k8s_reporting_owner",
	"k8s_metabase",
}

func TestInitialMigrationBuildsCompleteDatabase(t *testing.T) {
	maintenanceURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if maintenanceURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run PostgreSQL migration tests")
	}
	if os.Getenv(migratorIntegrationOptIn) != "1" {
		t.Skipf("set %s=1 to run against a dedicated empty PostgreSQL cluster", migratorIntegrationOptIn)
	}

	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	inspectionURL := maintenanceURLForDatabase(t, maintenanceURL, migrationURL)
	assertMigrationOwnerFixture(t, ctx, migrationURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}
	before := migrationLedgerRows(t, ctx, migrationURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatalf("second migration run must be a no-op: %v", err)
	}
	if after := migrationLedgerRows(t, ctx, migrationURL); !reflect.DeepEqual(after, before) {
		t.Fatalf("second migration run changed migration ledger: before=%+v after=%+v", before, after)
	}

	assertRoleContract(t, ctx, migrationURL)
	assertStaticCreatorAdminMemberships(t, ctx, migrationURL)
	bootstrapGrantor := currentRole(t, ctx, maintenanceURL)
	assertImplicitCreatorAdminMembership(t, ctx, migrationURL, "k8s_state_owner", bootstrapGrantor)
	assertImplicitCreatorAdminMembership(t, ctx, migrationURL, "k8s_reporting_owner", bootstrapGrantor)
	assertNoQueryBroker(t, ctx, migrationURL)
	assertOwnershipAndPublicACLs(t, ctx, inspectionURL, currentRole(t, ctx, migrationURL))
	assertRLSContract(t, ctx, inspectionURL)
	assertSecurityDefinerContract(t, ctx, inspectionURL)
	assertRuntimeLedgerAccess(t, ctx, credentials)

	tenantRole := "k8s_tenant_" + testToken(t)[:12]
	tenantPassword := testToken(t)
	t.Cleanup(func() {
		unregisterTenantRole(t, context.Background(), credentials.RoleAdmin, tenantRole)
		dropRole(t, context.Background(), maintenanceURL, tenantRole)
	})
	tenantURL := createTenantRolePath(t, ctx, credentials.RoleAdmin, tenantRole, tenantPassword, "tenant-alice")
	seedStateBoundaryData(t, ctx, inspectionURL)

	assertWriterBoundary(t, ctx, credentials.Writer)
	assertExporterBoundary(t, ctx, credentials.Exporter)
	assertTenantReadPath(t, ctx, tenantURL)
	assertMetabaseBoundary(t, ctx, credentials.Metabase)
	assertApplicationBoundary(t, ctx, credentials.Application)
}

func TestRunReconcilesStaticRoleContractDrift(t *testing.T) {
	maintenanceURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if maintenanceURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run PostgreSQL migration tests")
	}
	if os.Getenv(migratorIntegrationOptIn) != "1" {
		t.Skipf("set %s=1 to run against a dedicated empty PostgreSQL cluster", migratorIntegrationOptIn)
	}

	ctx := context.Background()
	adminURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: adminURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}
	connection := connect(t, ctx, adminURL)
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, `alter role cyclops_app createrole; grant k8s_query_tenant to k8s_role_admin with admin false, inherit true, set true`); err != nil {
		t.Fatal("introduce static role contract drift")
	}
	if err := Run(ctx, Config{MigrationURL: adminURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}
	assertRoleContract(t, ctx, adminURL)
}

func TestRunFailsClosedForUnsafeStaticRoleAttributeDrift(t *testing.T) {
	maintenanceURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if maintenanceURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run PostgreSQL migration tests")
	}
	if os.Getenv(migratorIntegrationOptIn) != "1" {
		t.Skipf("set %s=1 to run against a dedicated empty PostgreSQL cluster", migratorIntegrationOptIn)
	}

	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}
	maintenance := connect(t, ctx, maintenanceURL)
	defer maintenance.Close(ctx)
	if _, err := maintenance.Exec(ctx, `alter role cyclops_app superuser`); err != nil {
		t.Fatal("introduce unsafe static role drift")
	}

	err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials})
	if err == nil || !strings.Contains(err.Error(), "unsafe privileged drift") || !strings.Contains(err.Error(), "rolsuper=true") {
		t.Fatalf("Run() error = %v, want unsafe rolsuper drift fail-closed error", err)
	}
}

func TestRunFailsClosedForStaticRoleCreateDBDrift(t *testing.T) {
	maintenanceURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if maintenanceURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run PostgreSQL migration tests")
	}
	if os.Getenv(migratorIntegrationOptIn) != "1" {
		t.Skipf("set %s=1 to run against a dedicated empty PostgreSQL cluster", migratorIntegrationOptIn)
	}

	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}
	maintenance := connect(t, ctx, maintenanceURL)
	defer maintenance.Close(ctx)
	if _, err := maintenance.Exec(ctx, `alter role cyclops_app nologin createdb`); err != nil {
		t.Fatal("introduce static role createdb drift")
	}

	err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials})
	const want = "static role cyclops_app has unsafe privileged drift: rolcreatedb=true; the migration owner cannot safely repair these attributes"
	if err == nil || err.Error() != want {
		t.Fatalf("Run() error = %v, want %q", err, want)
	}

	var login, createDB bool
	if err := maintenance.QueryRow(ctx, `select rolcanlogin, rolcreatedb from pg_roles where rolname = 'cyclops_app'`).Scan(&login, &createDB); err != nil {
		t.Fatal("read static role after rejected createdb drift")
	}
	if login || !createDB {
		t.Fatalf("static role changed after rejected createdb drift: login:%t createdb:%t", login, createDB)
	}
}

func TestRunFailsClosedForForeignGrantorMembership(t *testing.T) {
	maintenanceURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if maintenanceURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run PostgreSQL migration tests")
	}
	if os.Getenv(migratorIntegrationOptIn) != "1" {
		t.Skipf("set %s=1 to run against a dedicated empty PostgreSQL cluster", migratorIntegrationOptIn)
	}

	ctx := context.Background()
	adminURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: adminURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}
	foreignGrantor := "foreign_grantor_" + testToken(t)[:12]
	connection := connect(t, ctx, adminURL)
	defer connection.Close(ctx)
	maintenance := connect(t, ctx, maintenanceURL)
	t.Cleanup(func() { maintenance.Close(context.Background()) })
	foreignIdentifier := pgx.Identifier{foreignGrantor}.Sanitize()
	t.Cleanup(func() {
		cleanupCtx := context.Background()
		if _, err := maintenance.Exec(cleanupCtx, "revoke k8s_query_tenant from k8s_role_admin granted by "+foreignIdentifier+" restrict"); err != nil {
			t.Errorf("revoke foreign-grantor static membership: %v", err)
		}
		if _, err := maintenance.Exec(cleanupCtx, "revoke k8s_query_tenant from "+foreignIdentifier+" restrict"); err != nil {
			t.Errorf("revoke foreign-grantor admin membership: %v", err)
		}
		dropRole(t, cleanupCtx, maintenanceURL, foreignGrantor)
	})
	if _, err := maintenance.Exec(ctx, "create role "+foreignIdentifier); err != nil {
		t.Fatal("create foreign grantor role")
	}
	if _, err := maintenance.Exec(ctx, "grant k8s_query_tenant to "+foreignIdentifier+" with admin true, inherit false, set false"); err != nil {
		t.Fatal("grant query tenant admin to foreign grantor")
	}
	if _, err := maintenance.Exec(ctx, "set role "+foreignIdentifier); err != nil {
		t.Fatal("set foreign grantor role")
	}
	if _, err := maintenance.Exec(ctx, `grant k8s_query_tenant to k8s_role_admin with admin true, inherit false, set false`); err != nil {
		t.Fatal("create foreign-grantor static membership")
	}
	if _, err := maintenance.Exec(ctx, `reset role`); err != nil {
		t.Fatal("reset foreign grantor role")
	}

	var membershipGrantCount, foreignGrantCount int
	if err := connection.QueryRow(ctx, `
		select count(*), count(*) filter (where grantor_role.rolname = $1)
		from pg_auth_members as membership
		join pg_roles as grantor_role on grantor_role.oid = membership.grantor
		where membership.roleid = 'k8s_query_tenant'::regrole
			and membership.member = 'k8s_role_admin'::regrole`, foreignGrantor).Scan(&membershipGrantCount, &foreignGrantCount); err != nil || membershipGrantCount != 2 || foreignGrantCount != 1 {
		t.Fatalf("static membership grantor rows = total:%d foreign:%d err=%v, want total:2 foreign:1", membershipGrantCount, foreignGrantCount, err)
	}

	err := Run(ctx, Config{MigrationURL: adminURL, Credentials: credentials})
	if err == nil || !strings.Contains(err.Error(), "has grantor") {
		t.Fatalf("Run() error = %v, want foreign-grantor fail-closed error", err)
	}
}

func TestRunFailsClosedForNonSuperuserStaticCreatorAdminGrantor(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	maintenance := connect(t, ctx, maintenanceURL)
	defer maintenance.Close(ctx)
	foreignGrantor := "foreign_creator_" + testToken(t)[:12]
	foreignIdentifier := pgx.Identifier{foreignGrantor}.Sanitize()
	migrationOwner := currentRole(t, ctx, migrationURL)
	migrationOwnerIdentifier := pgx.Identifier{migrationOwner}.Sanitize()
	if _, err := maintenance.Exec(ctx, "create role "+foreignIdentifier); err != nil {
		t.Fatal("create non-superuser foreign grantor")
	}
	if _, err := maintenance.Exec(ctx, "grant k8s_state_owner to "+foreignIdentifier+" with admin true, inherit false, set false"); err != nil {
		t.Fatal("grant static role admin to foreign grantor")
	}
	if _, err := maintenance.Exec(ctx, "set role "+foreignIdentifier); err != nil {
		t.Fatal("set non-superuser foreign grantor")
	}
	if _, err := maintenance.Exec(ctx, "grant k8s_state_owner to "+migrationOwnerIdentifier+" with admin true, inherit false, set false"); err != nil {
		t.Fatal("create non-superuser creator-shaped membership")
	}
	if _, err := maintenance.Exec(ctx, `reset role`); err != nil {
		t.Fatal("reset non-superuser foreign grantor")
	}
	if _, err := maintenance.Exec(ctx, `alter role cyclops_app inherit`); err != nil {
		t.Fatal("introduce static role repair sentinel")
	}

	assertRunFailsBeforeStaticRoleRepair(t, ctx, migrationURL, credentials, "true PostgreSQL superuser")
}

func TestRunFailsClosedForRegisteredTenantMembershipDrift(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()

	for _, testCase := range []struct {
		name      string
		introduce func(t *testing.T, maintenanceURL string, credentials CredentialURLs, tenantRole string)
	}{
		{
			name: "arbitrary inbound member",
			introduce: func(t *testing.T, _ string, credentials CredentialURLs, tenantRole string) {
				connection := connect(t, context.Background(), credentials.RoleAdmin)
				defer connection.Close(context.Background())
				arbitraryMember := pgx.Identifier{"arbitrary_tenant_member_" + testToken(t)[:12]}.Sanitize()
				if _, err := connection.Exec(context.Background(), "create role "+arbitraryMember); err != nil {
					t.Fatal("create arbitrary tenant member")
				}
				if _, err := connection.Exec(context.Background(), "grant "+pgx.Identifier{tenantRole}.Sanitize()+" to "+arbitraryMember+" with admin false, inherit true, set false"); err != nil {
					t.Fatal("grant registered tenant to arbitrary member")
				}
			},
		},
		{
			name: "extra parent role",
			introduce: func(t *testing.T, maintenanceURL string, _ CredentialURLs, tenantRole string) {
				connection := connect(t, context.Background(), maintenanceURL)
				defer connection.Close(context.Background())
				if _, err := connection.Exec(context.Background(), "grant pg_read_all_data to "+pgx.Identifier{tenantRole}.Sanitize()+" with admin false, inherit true, set false"); err != nil {
					t.Fatal("grant pg_read_all_data to registered tenant")
				}
			},
		},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
			if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
				t.Fatal(err)
			}
			tenantRole := "k8s_tenant_" + testToken(t)[:32]
			t.Cleanup(func() {
				unregisterTenantRole(t, context.Background(), credentials.RoleAdmin, tenantRole)
				dropRole(t, context.Background(), maintenanceURL, tenantRole)
			})
			createTenantRolePath(t, ctx, credentials.RoleAdmin, tenantRole, testToken(t), "tenant-registered")
			testCase.introduce(t, maintenanceURL, credentials, tenantRole)

			connection := connect(t, ctx, migrationURL)
			defer connection.Close(ctx)
			if _, err := connection.Exec(ctx, `alter role cyclops_app inherit`); err != nil {
				t.Fatal("introduce static role repair sentinel")
			}
			assertRunFailsBeforeStaticRoleRepair(t, ctx, migrationURL, credentials, "registered tenant")
		})
	}
}

func TestRunFailsClosedForUnexpectedExclusiveStaticRoleMember(t *testing.T) {
	maintenanceURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if maintenanceURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run PostgreSQL migration tests")
	}
	if os.Getenv(migratorIntegrationOptIn) != "1" {
		t.Skipf("set %s=1 to run against a dedicated empty PostgreSQL cluster", migratorIntegrationOptIn)
	}

	ctx := context.Background()
	adminURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: adminURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}
	unexpectedMember := "unexpected_member_" + testToken(t)[:12]
	connection := connect(t, ctx, adminURL)
	defer connection.Close(ctx)
	t.Cleanup(func() { dropRole(t, context.Background(), adminURL, unexpectedMember) })
	memberIdentifier := pgx.Identifier{unexpectedMember}.Sanitize()
	if _, err := connection.Exec(ctx, "create role "+memberIdentifier); err != nil {
		t.Fatal("create unexpected member role")
	}
	if _, err := connection.Exec(ctx, "grant k8s_query_admin to "+memberIdentifier); err != nil {
		t.Fatal("create unexpected exclusive static membership")
	}

	err := Run(ctx, Config{MigrationURL: adminURL, Credentials: credentials})
	if err == nil || !strings.Contains(err.Error(), "static role k8s_query_admin has unexpected members") {
		t.Fatalf("Run() error = %v, want unexpected-member fail-closed error", err)
	}
}

func TestRunFailsClosedForUnexpectedStaticRoleMembers(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()

	for _, role := range []string{"cyclops_app", "k8s_state_writer"} {
		t.Run(role, func(t *testing.T) {
			migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
			if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
				t.Fatal(err)
			}

			connection := connect(t, ctx, migrationURL)
			defer connection.Close(ctx)
			attacker := "unexpected_member_" + testToken(t)[:12]
			attackerIdentifier := pgx.Identifier{attacker}.Sanitize()
			t.Cleanup(func() { dropRole(t, context.Background(), maintenanceURL, attacker) })
			if _, err := connection.Exec(ctx, "create role "+attackerIdentifier); err != nil {
				t.Fatal("create unexpected static role member")
			}
			if _, err := connection.Exec(ctx, "grant "+pgx.Identifier{role}.Sanitize()+" to "+attackerIdentifier); err != nil {
				t.Fatal("grant unexpected static role membership")
			}
			if _, err := connection.Exec(ctx, `alter role cyclops_app inherit`); err != nil {
				t.Fatal("introduce repairable role drift")
			}

			err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials})
			if err == nil || !strings.Contains(err.Error(), "static role "+role+" has unexpected members") {
				t.Fatalf("Run() error = %v, want unexpected-member fail-closed error", err)
			}
			var inherit bool
			if err := connection.QueryRow(ctx, `select rolinherit from pg_roles where rolname = 'cyclops_app'`).Scan(&inherit); err != nil {
				t.Fatal("read role after rejected membership drift")
			}
			if !inherit {
				t.Fatal("role reconciliation changed a role before rejecting membership drift")
			}
		})
	}
}

func TestRunFailsClosedForUnexpectedStaticRoleParent(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	maintenance := connect(t, ctx, maintenanceURL)
	t.Cleanup(func() {
		cleanupConnection := connect(t, context.Background(), maintenanceURL)
		defer cleanupConnection.Close(context.Background())
		if _, err := cleanupConnection.Exec(context.Background(), `revoke pg_read_all_data from k8s_metabase restrict`); err != nil {
			t.Errorf("revoke unexpected static role parent: %v", err)
		}
	})
	defer maintenance.Close(ctx)
	if _, err := maintenance.Exec(ctx, `grant pg_read_all_data to k8s_metabase`); err != nil {
		t.Fatal("grant unexpected static role parent")
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, `alter role cyclops_app inherit`); err != nil {
		t.Fatal("introduce repairable role drift")
	}
	err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials})
	if err == nil || !strings.Contains(err.Error(), "static role k8s_metabase has unexpected parent roles pg_read_all_data") {
		t.Fatalf("Run() error = %v, want unexpected-parent fail-closed error", err)
	}
	var inherit bool
	if err := connection.QueryRow(ctx, `select rolinherit from pg_roles where rolname = 'cyclops_app'`).Scan(&inherit); err != nil {
		t.Fatal("read role after rejected parent drift")
	}
	if !inherit {
		t.Fatal("role reconciliation changed a role before rejecting parent drift")
	}
}

func TestRunAllowsRegisteredDynamicTenantMembership(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	tenantRole := "k8s_tenant_" + testToken(t)[:32]
	t.Cleanup(func() {
		unregisterTenantRole(t, context.Background(), credentials.RoleAdmin, tenantRole)
		dropRole(t, context.Background(), maintenanceURL, tenantRole)
	})
	createTenantRolePath(t, ctx, credentials.RoleAdmin, tenantRole, testToken(t), "tenant-registered")
	assertRegisteredTenantCreatorAdminMembership(t, ctx, migrationURL, tenantRole)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatalf("registered dynamic tenant membership must remain allowed: %v", err)
	}
}

func TestRunFailsClosedForUnregisteredDynamicTenantMembership(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	tenantRole := "k8s_tenant_" + testToken(t)[:32]
	tenantIdentifier := pgx.Identifier{tenantRole}.Sanitize()
	t.Cleanup(func() { dropRole(t, context.Background(), maintenanceURL, tenantRole) })
	if _, err := connection.Exec(ctx, "create role "+tenantIdentifier+" login inherit nocreaterole nocreatedb noreplication nobypassrls nosuperuser"); err != nil {
		t.Fatal("create unregistered tenant role")
	}
	if _, err := connection.Exec(ctx, "grant k8s_query_tenant to "+tenantIdentifier+" with admin false, inherit true, set false"); err != nil {
		t.Fatal("grant unregistered tenant membership")
	}
	err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials})
	if err == nil || !strings.Contains(err.Error(), "k8s_query_tenant has unregistered dynamic tenant member") {
		t.Fatalf("Run() error = %v, want unregistered-tenant fail-closed error", err)
	}
}

func TestRunReconcilesStaticRoleAvailabilityContract(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, `alter role cyclops_app connection limit 1 valid until '2000-01-01 00:00:00+00'`); err != nil {
		t.Fatal("introduce static role availability drift")
	}
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatalf("reconcile static role availability drift: %v", err)
	}
	var connectionLimit int
	var validUntil string
	if err := connection.QueryRow(ctx, `select rolconnlimit, coalesce(rolvaliduntil::text, 'infinity') from pg_roles where rolname = 'cyclops_app'`).Scan(&connectionLimit, &validUntil); err != nil {
		t.Fatal("read reconciled static role availability")
	}
	if connectionLimit != -1 || validUntil != "infinity" {
		t.Fatalf("static role availability = connection_limit:%d valid_until:%q, want connection_limit:-1 valid_until:infinity", connectionLimit, validUntil)
	}
}

func TestRunReconcilesStaticRoleSettingsContract(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	for _, statement := range []string{
		`alter role k8s_metabase set default_transaction_read_only = off`,
		`alter role k8s_metabase set search_path = public`,
		`alter role k8s_metabase in database ` + pgx.Identifier{connection.Config().Database}.Sanitize() + ` set work_mem = '64MB'`,
	} {
		if _, err := connection.Exec(ctx, statement); err != nil {
			t.Fatalf("introduce static role setting drift %q: %v", statement, err)
		}
	}
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatalf("reconcile static role setting drift: %v", err)
	}
	assertStaticRoleSettings(t, ctx, connection, "k8s_metabase", map[string]string{
		"default_transaction_read_only":       "on",
		"statement_timeout":                   "20000ms",
		"idle_in_transaction_session_timeout": "20000ms",
	})
	for _, role := range staticMigrationRoles {
		if role == "k8s_metabase" {
			continue
		}
		assertStaticRoleSettings(t, ctx, connection, role, map[string]string{})
	}
}

func requireMigratorIntegration(t *testing.T) string {
	t.Helper()
	maintenanceURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if maintenanceURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run PostgreSQL migration tests")
	}
	if os.Getenv(migratorIntegrationOptIn) != "1" {
		t.Skipf("set %s=1 to run against a dedicated empty PostgreSQL cluster", migratorIntegrationOptIn)
	}
	return maintenanceURL
}

func assertRegisteredTenantCreatorAdminMembership(t *testing.T, ctx context.Context, migrationURL, tenantRole string) {
	t.Helper()
	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	rows, err := connection.Query(ctx, `
		select grantor_role.rolname, membership.admin_option, membership.inherit_option, membership.set_option
		from pg_auth_members as membership
		join pg_roles as grantor_role on grantor_role.oid = membership.grantor
		where membership.roleid = $1::regrole and membership.member = 'k8s_role_admin'::regrole
		order by grantor_role.rolname`, tenantRole)
	if err != nil {
		t.Fatalf("read tenant creator-admin membership: %v", err)
	}
	defer rows.Close()

	count := 0
	for rows.Next() {
		var grantor string
		var admin, inherit, set bool
		if err := rows.Scan(&grantor, &admin, &inherit, &set); err != nil {
			t.Fatalf("scan tenant creator-admin membership: %v", err)
		}
		if grantor == "k8s_role_admin" || !admin || inherit || set {
			t.Fatalf("tenant creator-admin membership = grantor:%s admin:%t inherit:%t set:%t, want foreign grantor and admin:true inherit:false set:false", grantor, admin, inherit, set)
		}
		count++
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("iterate tenant creator-admin membership: %v", err)
	}
	if count == 0 {
		t.Fatal("registered tenant is missing its PG16 creator-admin membership for k8s_role_admin")
	}
}

func assertStaticRoleSettings(t *testing.T, ctx context.Context, connection *pgx.Conn, role string, want map[string]string) {
	t.Helper()
	rows, err := connection.Query(ctx, `
		select setting.setdatabase, setting.setconfig
		from pg_db_role_setting as setting
		join pg_roles as configured_role on configured_role.oid = setting.setrole
		where configured_role.rolname = $1
		order by setting.setdatabase`, role)
	if err != nil {
		t.Fatalf("read role settings for %s: %v", role, err)
	}
	defer rows.Close()

	got := map[string]string{}
	for rows.Next() {
		var databaseOID uint32
		var settings []string
		if err := rows.Scan(&databaseOID, &settings); err != nil {
			t.Fatalf("scan role settings for %s: %v", role, err)
		}
		if databaseOID != 0 {
			t.Fatalf("role %s has database-specific settings for database OID %d: %v", role, databaseOID, settings)
		}
		for _, setting := range settings {
			name, value, ok := strings.Cut(setting, "=")
			if !ok {
				t.Fatalf("role %s has malformed setting %q", role, setting)
			}
			got[name] = value
		}
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("iterate role settings for %s: %v", role, err)
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("role %s settings = %#v, want %#v", role, got, want)
	}
}

type ledgerRow struct {
	Version          int64
	ApplicationOrder int64
	Filename         string
	SHA256           string
	AppliedAt        time.Time
}

func isolatedMigrationDatabase(t *testing.T, ctx context.Context, maintenanceURL string) (string, CredentialURLs) {
	t.Helper()
	parsed, err := url.Parse(maintenanceURL)
	if err != nil {
		t.Fatal("parse CYCLOPS_TEST_DATABASE_URL")
	}
	if parsed.Scheme != "postgres" && parsed.Scheme != "postgresql" {
		t.Fatal("CYCLOPS_TEST_DATABASE_URL must be a PostgreSQL URL")
	}
	if parsed.Path != "/postgres" {
		t.Fatalf("%s=1 requires CYCLOPS_TEST_DATABASE_URL to use the /postgres maintenance database", migratorIntegrationOptIn)
	}

	maintenance, err := pgx.Connect(ctx, maintenanceURL)
	if err != nil {
		t.Fatal("connect maintenance database")
	}
	if _, err := maintenance.Exec(ctx, `select pg_advisory_lock(hashtext($1))`, "cyclops-database-migrator-integration"); err != nil {
		maintenance.Close(ctx)
		t.Fatal("lock migration integration setup")
	}

	var existingDatabases []string
	if err := maintenance.QueryRow(ctx, `
		select coalesce(array_agg(datname order by datname), '{}')
		from pg_database
		where datallowconn and not datistemplate and datname <> 'postgres'`).Scan(&existingDatabases); err != nil {
		maintenance.Close(ctx)
		t.Fatal("verify isolated PostgreSQL cluster databases")
	}
	if len(existingDatabases) != 0 {
		maintenance.Close(ctx)
		t.Fatalf("%s=1 requires an empty PostgreSQL cluster; found connectable databases: %s", migratorIntegrationOptIn, strings.Join(existingDatabases, ", "))
	}

	var existing []string
	if err := maintenance.QueryRow(ctx, `select coalesce(array_agg(rolname order by rolname), '{}') from pg_roles where rolname = any($1)`, staticMigrationRoles).Scan(&existing); err != nil {
		maintenance.Close(ctx)
		t.Fatal("check static migration roles")
	}
	if len(existing) != 0 {
		maintenance.Close(ctx)
		t.Fatalf("%s=1 requires no pre-existing static migration roles; found: %s", migratorIntegrationOptIn, strings.Join(existing, ", "))
	}

	migrationOwner := "cyclops_migrator_" + testToken(t)[:12]
	migrationPassword := testToken(t)
	migrationOwnerIdentifier := pgx.Identifier{migrationOwner}.Sanitize()
	if _, err := maintenance.Exec(ctx, "create role "+migrationOwnerIdentifier+" login createrole password "+quoteLiteral(migrationPassword)); err != nil {
		maintenance.Close(ctx)
		t.Fatal("create isolated migration owner")
	}

	databaseName := "cyclops_migrate_" + testToken(t)[:16]
	if _, err := maintenance.Exec(ctx, "create database "+pgx.Identifier{databaseName}.Sanitize()+" owner "+migrationOwnerIdentifier); err != nil {
		maintenance.Close(ctx)
		t.Fatal("create isolated migration database")
	}
	t.Cleanup(func() {
		cleanupCtx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		if _, err := maintenance.Exec(cleanupCtx, "drop database if exists "+pgx.Identifier{databaseName}.Sanitize()+" with (force)"); err != nil {
			t.Errorf("drop isolated migration database: %v", err)
		}
		for _, role := range staticMigrationRoles {
			if _, err := maintenance.Exec(cleanupCtx, "drop role if exists "+pgx.Identifier{role}.Sanitize()); err != nil {
				t.Errorf("drop static migration role %s: %v", role, err)
			}
		}
		if _, err := maintenance.Exec(cleanupCtx, "drop role if exists "+migrationOwnerIdentifier); err != nil {
			t.Errorf("drop isolated migration owner: %v", err)
		}
		maintenance.Close(cleanupCtx)
	})

	parsed.Path = "/" + databaseName
	parsed.RawPath = ""
	parsed.User = url.UserPassword(migrationOwner, migrationPassword)
	migrationURL := parsed.String()
	return migrationURL, testCredentialURLs(t, migrationURL)
}

func maintenanceURLForDatabase(t *testing.T, maintenanceURL, databaseURL string) string {
	t.Helper()
	maintenance, err := url.Parse(maintenanceURL)
	if err != nil {
		t.Fatal("parse maintenance database URL")
	}
	database, err := url.Parse(databaseURL)
	if err != nil {
		t.Fatal("parse migration database URL")
	}
	maintenance.Path = database.Path
	maintenance.RawPath = database.RawPath
	return maintenance.String()
}

func quoteLiteral(value string) string {
	return "'" + strings.ReplaceAll(value, "'", "''") + "'"
}

func testCredentialURLs(t *testing.T, adminURL string) CredentialURLs {
	t.Helper()
	parsed, err := url.Parse(adminURL)
	if err != nil {
		t.Fatal("parse migration database URL")
	}
	withRole := func(role string) string {
		copy := *parsed
		copy.User = url.UserPassword(role, testToken(t))
		return copy.String()
	}
	return CredentialURLs{
		Application: withRole("cyclops_app"),
		Writer:      withRole("k8s_state_writer"),
		Exporter:    withRole("k8s_state_exporter"),
		RoleAdmin:   withRole("k8s_role_admin"),
		Metabase:    withRole("k8s_metabase"),
	}
}

func testToken(t *testing.T) string {
	t.Helper()
	bytes := make([]byte, 16)
	if _, err := rand.Read(bytes); err != nil {
		t.Fatal("generate test database token")
	}
	return hex.EncodeToString(bytes)
}

func migrationLedgerRows(t *testing.T, ctx context.Context, adminURL string) []ledgerRow {
	t.Helper()
	connection := connect(t, ctx, adminURL)
	defer connection.Close(ctx)
	rows, err := connection.Query(ctx, `select version, application_order, filename, sha256, applied_at from cyclops_migrations.applied_migrations order by application_order`)
	if err != nil {
		t.Fatal("read migration ledger")
	}
	defer rows.Close()
	var ledger []ledgerRow
	for rows.Next() {
		var row ledgerRow
		if err := rows.Scan(&row.Version, &row.ApplicationOrder, &row.Filename, &row.SHA256, &row.AppliedAt); err != nil {
			t.Fatal("scan migration ledger")
		}
		ledger = append(ledger, row)
	}
	if err := rows.Err(); err != nil {
		t.Fatal("iterate migration ledger")
	}
	if len(ledger) != 1 {
		t.Fatalf("migration ledger row count = %d, want 1", len(ledger))
	}
	return ledger
}

func assertMigrationOwnerFixture(t *testing.T, ctx context.Context, migrationURL string) {
	t.Helper()
	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	var createRole, super bool
	if err := connection.QueryRow(ctx, `select rolcreaterole, rolsuper from pg_roles where rolname = current_user`).Scan(&createRole, &super); err != nil {
		t.Fatal("read isolated migration owner attributes")
	}
	if !createRole || super {
		t.Fatalf("isolated migration owner attributes = createrole:%t super:%t, want createrole:true super:false", createRole, super)
	}
}

func currentRole(t *testing.T, ctx context.Context, databaseURL string) string {
	t.Helper()
	connection := connect(t, ctx, databaseURL)
	defer connection.Close(ctx)
	var role string
	if err := connection.QueryRow(ctx, `select current_user`).Scan(&role); err != nil {
		t.Fatal("read current database role")
	}
	return role
}

func assertStaticCreatorAdminMemberships(t *testing.T, ctx context.Context, migrationURL string) {
	t.Helper()
	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	migrationOwner := currentRole(t, ctx, migrationURL)
	for _, role := range staticMigrationRoles {
		rows, err := connection.Query(ctx, `
			select grantor_role.rolname, grantor_role.rolsuper, membership.admin_option, membership.inherit_option, membership.set_option
			from pg_auth_members as membership
			join pg_roles as grantor_role on grantor_role.oid = membership.grantor
			where membership.roleid = $1::regrole and membership.member = current_user::regrole
			order by grantor_role.rolname`, role)
		if err != nil {
			t.Fatalf("read static creator memberships for %s: %v", role, err)
		}
		var grants []staticMembershipGrant
		for rows.Next() {
			var grant staticMembershipGrant
			if err := rows.Scan(&grant.grantor, &grant.grantorSuperuser, &grant.admin, &grant.inherit, &grant.set); err != nil {
				rows.Close()
				t.Fatalf("scan static creator membership for %s: %v", role, err)
			}
			grants = append(grants, grant)
		}
		if err := rows.Err(); err != nil {
			rows.Close()
			t.Fatalf("iterate static creator memberships for %s: %v", role, err)
		}
		rows.Close()
		allowOwnerGrant := role == "k8s_state_owner" || role == "k8s_reporting_owner"
		if !staticCreatorAdminMembershipsAreExact(migrationOwner, grants, allowOwnerGrant) {
			t.Fatalf("static creator memberships for %s = %+v", role, grants)
		}
	}
}

func assertImplicitCreatorAdminMembership(t *testing.T, ctx context.Context, migrationURL, role, bootstrapGrantor string) {
	t.Helper()
	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	rows, err := connection.Query(ctx, `
		select grantor_role.rolname, membership.admin_option, membership.inherit_option, membership.set_option
		from pg_auth_members as membership
		join pg_roles as grantor_role on grantor_role.oid = membership.grantor
		where membership.roleid = $1::regrole
			and membership.member = current_user::regrole
		order by grantor_role.rolname`, role)
	if err != nil {
		t.Fatalf("read creator-admin memberships for %s: %v", role, err)
	}
	defer rows.Close()
	var memberships []struct {
		grantor             string
		admin, inherit, set bool
	}
	for rows.Next() {
		var membership struct {
			grantor             string
			admin, inherit, set bool
		}
		if err := rows.Scan(&membership.grantor, &membership.admin, &membership.inherit, &membership.set); err != nil {
			t.Fatalf("scan creator-admin membership for %s: %v", role, err)
		}
		memberships = append(memberships, membership)
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("iterate creator-admin memberships for %s: %v", role, err)
	}
	currentUser := currentRole(t, ctx, migrationURL)
	want := map[string]struct{ admin, inherit, set bool }{
		bootstrapGrantor: {true, false, false},
		currentUser:      {false, false, true},
	}
	if len(memberships) != len(want) {
		t.Fatalf("creator-admin membership count for %s = %d, want %d (%+v)", role, len(memberships), len(want), memberships)
	}
	for _, membership := range memberships {
		expected, ok := want[membership.grantor]
		if !ok || membership.admin != expected.admin || membership.inherit != expected.inherit || membership.set != expected.set {
			t.Fatalf("creator-admin membership for %s = %+v, want %+v", role, membership, want)
		}
		delete(want, membership.grantor)
	}
	if len(want) != 0 {
		t.Fatalf("creator-admin membership for %s omitted %+v", role, want)
	}
}

func assertRoleContract(t *testing.T, ctx context.Context, adminURL string) {
	t.Helper()
	connection := connect(t, ctx, adminURL)
	defer connection.Close(ctx)

	expected := map[string]struct{ login, inherit, createRole bool }{
		"cyclops_app":         {true, false, false},
		"k8s_state_owner":     {false, true, false},
		"k8s_state_writer":    {true, false, false},
		"k8s_state_exporter":  {true, false, false},
		"k8s_query_tenant":    {false, true, false},
		"k8s_query_admin":     {false, true, false},
		"k8s_role_admin":      {true, false, true},
		"k8s_reporting_owner": {false, true, false},
		"k8s_metabase":        {true, false, false},
	}
	for role, want := range expected {
		var login, inherit, createRole, super, createDB, replication, bypassRLS bool
		if err := connection.QueryRow(ctx, `select rolcanlogin, rolinherit, rolcreaterole, rolsuper, rolcreatedb, rolreplication, rolbypassrls from pg_roles where rolname = $1`, role).Scan(&login, &inherit, &createRole, &super, &createDB, &replication, &bypassRLS); err != nil {
			t.Fatalf("read role %s: %v", role, err)
		}
		if login != want.login || inherit != want.inherit || createRole != want.createRole || super || createDB || replication || bypassRLS {
			t.Errorf("role %s attributes = login:%t inherit:%t createrole:%t super:%t createdb:%t replication:%t bypassrls:%t", role, login, inherit, createRole, super, createDB, replication, bypassRLS)
		}
	}

	var currentUser string
	if err := connection.QueryRow(ctx, `select current_user`).Scan(&currentUser); err != nil {
		t.Fatal("read migration user")
	}
	for _, want := range []struct {
		role, member        string
		admin, inherit, set bool
	}{
		{"k8s_state_owner", currentUser, false, false, true},
		{"k8s_reporting_owner", currentUser, false, false, true},
		{"k8s_query_tenant", "k8s_role_admin", true, false, false},
		{"k8s_query_admin", "k8s_reporting_owner", false, true, false},
	} {
		rows, err := connection.Query(ctx, `
			select grantor_role.rolname, membership.admin_option, membership.inherit_option, membership.set_option
			from pg_auth_members as membership
			join pg_roles as grantor_role on grantor_role.oid = membership.grantor
			where membership.roleid = $1::regrole and membership.member = $2::regrole
			order by grantor_role.rolname`, want.role, want.member)
		if err != nil {
			t.Errorf("read membership %s -> %s: %v", want.role, want.member, err)
			continue
		}

		var grants []struct {
			grantor             string
			admin, inherit, set bool
		}
		for rows.Next() {
			var grant struct {
				grantor             string
				admin, inherit, set bool
			}
			if err := rows.Scan(&grant.grantor, &grant.admin, &grant.inherit, &grant.set); err != nil {
				t.Errorf("scan membership %s -> %s: %v", want.role, want.member, err)
				break
			}
			grants = append(grants, grant)
		}
		if err := rows.Err(); err != nil {
			t.Errorf("iterate membership %s -> %s: %v", want.role, want.member, err)
		}
		rows.Close()

		var explicitGrantFound bool
		var bootstrapGrantCount int
		for _, grant := range grants {
			if grant.grantor == currentUser {
				if explicitGrantFound || grant.admin != want.admin || grant.inherit != want.inherit || grant.set != want.set {
					t.Errorf("membership %s -> %s = grantor:%s admin:%t inherit:%t set:%t, want one explicit grantor:%s admin:%t inherit:%t set:%t", want.role, want.member, grant.grantor, grant.admin, grant.inherit, grant.set, currentUser, want.admin, want.inherit, want.set)
				}
				explicitGrantFound = true
				continue
			}
			if want.member == currentUser && grant.admin && !grant.inherit && !grant.set {
				bootstrapGrantCount++
				continue
			}
			t.Errorf("membership %s -> %s has unexpected grantor:%s admin:%t inherit:%t set:%t", want.role, want.member, grant.grantor, grant.admin, grant.inherit, grant.set)
		}
		if !explicitGrantFound {
			t.Errorf("membership %s -> %s is missing the explicit migration-owner grant", want.role, want.member)
		}
		wantBootstrapGrants := 0
		if want.member == currentUser {
			wantBootstrapGrants = 1
		}
		if bootstrapGrantCount != wantBootstrapGrants {
			t.Errorf("membership %s -> %s has %d bootstrap grantor rows, want %d", want.role, want.member, bootstrapGrantCount, wantBootstrapGrants)
		}
	}
}

func assertNoQueryBroker(t *testing.T, ctx context.Context, adminURL string) {
	t.Helper()
	connection := connect(t, ctx, adminURL)
	defer connection.Close(ctx)
	var exists bool
	if err := connection.QueryRow(ctx, `select exists(select 1 from pg_roles where rolname = 'k8s_query_broker')`).Scan(&exists); err != nil || exists {
		t.Errorf("shared query broker exists=%t err=%v", exists, err)
	}
}

func assertOwnershipAndPublicACLs(t *testing.T, ctx context.Context, adminURL, migrationOwner string) {
	t.Helper()
	connection := connect(t, ctx, adminURL)
	defer connection.Close(ctx)
	for _, want := range []struct{ schema, name, owner string }{
		{"public", "github_trust_policies", migrationOwner},
		{"k8s_state", "resource_state", "k8s_state_owner"},
		{"k8s_state", "resource_event_outbox", "k8s_state_owner"},
		{"k8s_api", "current_resources", "k8s_state_owner"},
		{"k8s_reporting", "current_resources", "k8s_reporting_owner"},
	} {
		var owner string
		if err := connection.QueryRow(ctx, `select relation.relowner::regrole::text from pg_class relation join pg_namespace namespace on namespace.oid = relation.relnamespace where namespace.nspname = $1 and relation.relname = $2`, want.schema, want.name).Scan(&owner); err != nil {
			t.Errorf("read owner for %s.%s: %v", want.schema, want.name, err)
		} else if owner != want.owner {
			t.Errorf("owner for %s.%s = %s, want %s", want.schema, want.name, owner, want.owner)
		}
	}
	for _, want := range []struct{ schema, owner string }{
		{"k8s_state", "k8s_state_owner"},
		{"k8s_api", "k8s_state_owner"},
		{"k8s_reporting", "k8s_reporting_owner"},
		{"cyclops_migrations", migrationOwner},
	} {
		var owner string
		if err := connection.QueryRow(ctx, `select nspowner::regrole::text from pg_namespace where nspname = $1`, want.schema).Scan(&owner); err != nil {
			t.Errorf("read schema owner for %s: %v", want.schema, err)
		} else if owner != want.owner {
			t.Errorf("schema owner for %s = %s, want %s", want.schema, owner, want.owner)
		}
	}
	for _, schema := range []string{"public", "k8s_state", "k8s_api", "k8s_reporting", "cyclops_migrations"} {
		assertNoPublicSchemaPrivilege(t, ctx, connection, schema, "CREATE")
	}
	for _, schema := range []string{"k8s_state", "k8s_api", "k8s_reporting", "cyclops_migrations"} {
		assertNoPublicSchemaPrivilege(t, ctx, connection, schema, "USAGE")
	}
	for _, relation := range []string{"public.github_trust_policies", "k8s_state.resource_state", "k8s_state.resource_event_outbox", "k8s_api.current_resources", "k8s_reporting.current_resources", "cyclops_migrations.applied_migrations"} {
		assertNoPublicTablePrivilege(t, ctx, connection, relation)
	}
}

func assertRLSContract(t *testing.T, ctx context.Context, adminURL string) {
	t.Helper()
	connection := connect(t, ctx, adminURL)
	defer connection.Close(ctx)
	var enabled, forced bool
	if err := connection.QueryRow(ctx, `select relrowsecurity, relforcerowsecurity from pg_class where oid = 'k8s_state.resource_state'::regclass`).Scan(&enabled, &forced); err != nil || !enabled || !forced {
		t.Errorf("resource_state RLS flags = enabled:%t forced:%t err=%v", enabled, forced, err)
	}
	var predicate string
	if err := connection.QueryRow(ctx, `select pg_get_expr(policy.polqual, policy.polrelid) from pg_policy policy where policy.polname = 'tenant_current_state' and policy.polrelid = 'k8s_state.resource_state'::regclass`).Scan(&predicate); err != nil {
		t.Errorf("read tenant RLS predicate: %v", err)
	} else if !strings.Contains(predicate, "capsule_tenant = k8s_state.tenant_for_role(CURRENT_USER)") || !strings.Contains(predicate, "namespace") || !strings.Contains(predicate, "resource = 'namespaces'") {
		t.Errorf("tenant RLS predicate = %q", predicate)
	}
}

func assertSecurityDefinerContract(t *testing.T, ctx context.Context, adminURL string) {
	t.Helper()
	connection := connect(t, ctx, adminURL)
	defer connection.Close(ctx)
	for _, function := range []struct {
		identity, allowedRole string
	}{
		{"k8s_state.tenant_for_role(name)", "k8s_query_tenant"},
		{"k8s_state.register_tenant_role(name,text,text)", "k8s_role_admin"},
		{"k8s_state.unregister_tenant_role(name)", "k8s_role_admin"},
	} {
		var securityDefiner bool
		var config string
		var owner string
		if err := connection.QueryRow(ctx, `select prosecdef, coalesce(array_to_string(proconfig, ','), ''), proowner::regrole::text from pg_proc where oid = $1::regprocedure`, function.identity).Scan(&securityDefiner, &config, &owner); err != nil {
			t.Fatalf("read function %s: %v", function.identity, err)
		}
		if !securityDefiner || owner != "k8s_state_owner" || !strings.Contains(config, "search_path=k8s_state, pg_catalog") {
			t.Errorf("function %s security contract = definer:%t owner:%s config:%q", function.identity, securityDefiner, owner, config)
		}
		assertNoPublicFunctionExecute(t, ctx, connection, function.identity)
		var allowed bool
		if err := connection.QueryRow(ctx, `select has_function_privilege($1, $2::regprocedure, 'EXECUTE')`, function.allowedRole, function.identity).Scan(&allowed); err != nil || !allowed {
			t.Errorf("role %s cannot execute %s: allowed=%t err=%v", function.allowedRole, function.identity, allowed, err)
		}
	}
}

func assertRuntimeLedgerAccess(t *testing.T, ctx context.Context, credentials CredentialURLs) {
	t.Helper()
	for role, databaseURL := range map[string]string{
		"application": credentials.Application,
		"writer":      credentials.Writer,
		"exporter":    credentials.Exporter,
		"role-admin":  credentials.RoleAdmin,
		"metabase":    credentials.Metabase,
	} {
		connection := connect(t, ctx, databaseURL)
		var count int
		err := connection.QueryRow(ctx, `select count(*) from cyclops_migrations.applied_migrations`).Scan(&count)
		connection.Close(ctx)
		if err != nil || count != 1 {
			t.Errorf("%s ledger select = count:%d err:%v", role, count, err)
		}
		assertStatementFails(t, ctx, databaseURL, `insert into cyclops_migrations.applied_migrations (version, filename, sha256) values (99, 'invalid.sql', 'invalid')`)
		assertStatementFails(t, ctx, databaseURL, `update cyclops_migrations.applied_migrations set filename = 'invalid.sql' where version = 1`)
		assertStatementFails(t, ctx, databaseURL, `delete from cyclops_migrations.applied_migrations where version = 1`)
	}
}

func createTenantRolePath(t *testing.T, ctx context.Context, roleAdminURL, tenantRole, tenantPassword, tenant string) string {
	t.Helper()
	connection := connect(t, ctx, roleAdminURL)
	defer connection.Close(ctx)
	transaction, err := connection.Begin(ctx)
	if err != nil {
		t.Fatal("begin tenant role transaction")
	}
	defer transaction.Rollback(ctx)
	identifier := pgx.Identifier{tenantRole}.Sanitize()
	var quotedPassword string
	if err := transaction.QueryRow(ctx, `select format('%L', $1::text)`, tenantPassword).Scan(&quotedPassword); err != nil {
		t.Fatal("quote tenant role password")
	}
	for _, statement := range []string{
		"create role " + identifier + " login inherit nocreaterole nocreatedb noreplication nobypassrls nosuperuser password " + quotedPassword,
		"grant k8s_query_tenant to " + identifier + " with admin false, inherit true, set false",
	} {
		if _, err := transaction.Exec(ctx, statement); err != nil {
			t.Fatalf("role admin expected tenant path %q: %v", statement, err)
		}
	}
	if _, err := transaction.Exec(ctx, `select k8s_state.register_tenant_role($1, $2, $3)`, tenantRole, tenant, "test-fingerprint"); err != nil {
		t.Fatalf("role admin register tenant role: %v", err)
	}
	if err := transaction.Commit(ctx); err != nil {
		t.Fatal("commit tenant role transaction")
	}
	var login, inherit, createRole, super, createDB, replication, bypassRLS bool
	if err := connection.QueryRow(ctx, `select rolcanlogin, rolinherit, rolcreaterole, rolsuper, rolcreatedb, rolreplication, rolbypassrls from pg_roles where rolname = $1`, tenantRole).Scan(&login, &inherit, &createRole, &super, &createDB, &replication, &bypassRLS); err != nil {
		t.Fatal("read tenant role attributes")
	}
	if !login || !inherit || createRole || super || createDB || replication || bypassRLS {
		t.Fatalf("tenant role attributes = login:%t inherit:%t createrole:%t super:%t createdb:%t replication:%t bypassrls:%t", login, inherit, createRole, super, createDB, replication, bypassRLS)
	}
	var admin, membershipInherit, set bool
	if err := connection.QueryRow(ctx, `select admin_option, inherit_option, set_option from pg_auth_members where roleid = 'k8s_query_tenant'::regrole and member = $1::regrole`, tenantRole).Scan(&admin, &membershipInherit, &set); err != nil {
		t.Fatal("read tenant query membership")
	}
	if admin || !membershipInherit || set {
		t.Fatalf("tenant query membership = admin:%t inherit:%t set:%t, want admin:false inherit:true set:false", admin, membershipInherit, set)
	}
	var fingerprint string
	if err := connection.QueryRow(ctx, tenantCredentialFingerprintLookup, tenantRole).Scan(&fingerprint); err != nil || fingerprint != "test-fingerprint" {
		t.Fatalf("registered tenant fingerprint = %q err=%v", fingerprint, err)
	}
	assertStatementFails(t, ctx, roleAdminURL, "grant k8s_query_admin to "+identifier)
	parsed, err := url.Parse(roleAdminURL)
	if err != nil {
		t.Fatal("parse role-admin database URL")
	}
	parsed.User = url.UserPassword(tenantRole, tenantPassword)
	return parsed.String()
}

func seedStateBoundaryData(t *testing.T, ctx context.Context, adminURL string) {
	t.Helper()
	connection := connect(t, ctx, adminURL)
	defer connection.Close(ctx)
	_, err := connection.Exec(ctx, `
		insert into k8s_state.resource_state
		(cluster_id, api_group, resource, namespace, name, capsule_tenant, schema_hash, watch_epoch, observed_sequence, labels, object)
		values
		('migration-test', '', 'namespaces', '', 'alice-ns', 'tenant-alice', 'schema', 1, 1, '{}', '{"kind":"Namespace"}'),
		('migration-test', '', 'pods', 'alice-ns', 'pod-a', 'tenant-alice', 'schema', 1, 2, '{}', '{"kind":"Pod"}'),
		('migration-test', '', 'nodes', '', 'node-a', 'tenant-alice', 'schema', 1, 3, '{}', '{"kind":"Node"}'),
		('migration-test', '', 'pods', 'bob-ns', 'pod-b', 'tenant-bob', 'schema', 1, 4, '{}', '{"kind":"Pod"}');
		insert into k8s_state.resource_event_outbox
		(event_id, cluster_id, api_group, resource, namespace, name, capsule_tenant, schema_hash, event_type, watch_epoch, observed_sequence, object, observed_at)
		values ('00000000-0000-0000-0000-000000000001', 'migration-test', '', 'pods', 'alice-ns', 'pod-a', 'tenant-alice', 'schema', 'ADDED', 1, 2, '{}', clock_timestamp())`)
	if err != nil {
		t.Fatal("seed access-boundary state")
	}
}

func assertWriterBoundary(t *testing.T, ctx context.Context, writerURL string) {
	t.Helper()
	connection := connect(t, ctx, writerURL)
	defer connection.Close(ctx)
	transaction, err := connection.Begin(ctx)
	if err != nil {
		t.Fatal("begin writer transaction")
	}
	defer transaction.Rollback(ctx)
	_, err = transaction.Exec(ctx, `insert into k8s_state.resource_state (cluster_id, api_group, resource, namespace, name, schema_hash, watch_epoch, observed_sequence, labels, object) values ('writer-test', '', 'pods', 'default', 'pod', 'schema', 1, 1, '{}', '{}')`)
	if err != nil {
		t.Fatalf("writer cannot write current state: %v", err)
	}
	assertStatementFails(t, ctx, writerURL, `insert into k8s_state.query_tenant_role (role_name, capsule_tenant, credential_fingerprint) values ('writer_forbidden', 'writer', 'forbidden')`)
	assertStatementFails(t, ctx, writerURL, `set role k8s_state_owner`)
}

func assertExporterBoundary(t *testing.T, ctx context.Context, exporterURL string) {
	t.Helper()
	connection := connect(t, ctx, exporterURL)
	defer connection.Close(ctx)
	var count int
	if err := connection.QueryRow(ctx, `select count(*) from k8s_state.resource_event_outbox where cluster_id = 'migration-test'`).Scan(&count); err != nil || count != 1 {
		t.Errorf("exporter cannot read outbox boundary: count=%d err=%v", count, err)
	}
	transaction, err := connection.Begin(ctx)
	if err != nil {
		t.Fatal("begin exporter transaction")
	}
	defer transaction.Rollback(ctx)
	if _, err := transaction.Exec(ctx, `update k8s_state.resource_event_outbox set claimed_at = clock_timestamp() where event_id = '00000000-0000-0000-0000-000000000001'`); err != nil {
		t.Errorf("exporter cannot claim outbox event: %v", err)
	}
	assertStatementFails(t, ctx, exporterURL, `select 1 from k8s_state.resource_state limit 1`)
}

func assertTenantReadPath(t *testing.T, ctx context.Context, tenantURL string) {
	t.Helper()
	if names := tenantResourceNames(t, ctx, tenantURL); strings.Join(names, ",") != "alice-ns,pod-a" {
		t.Errorf("tenant RLS predicate results = %v, want [alice-ns pod-a]", names)
	}
}

func tenantResourceNames(t *testing.T, ctx context.Context, tenantURL string) []string {
	t.Helper()
	connection := connect(t, ctx, tenantURL)
	defer connection.Close(ctx)
	rows, err := connection.Query(ctx, `select name from k8s_api.current_resources where cluster_id = 'migration-test' order by name`)
	if err != nil {
		t.Fatalf("query directly as tenant role: %v", err)
	}
	defer rows.Close()
	var names []string
	for rows.Next() {
		var name string
		if err := rows.Scan(&name); err != nil {
			t.Fatal("scan tenant result")
		}
		names = append(names, name)
	}
	if err := rows.Err(); err != nil {
		t.Fatal("read tenant result")
	}
	return names
}

func assertMetabaseBoundary(t *testing.T, ctx context.Context, metabaseURL string) {
	t.Helper()
	connection := connect(t, ctx, metabaseURL)
	defer connection.Close(ctx)
	var count int
	if err := connection.QueryRow(ctx, `select count(*) from k8s_reporting.current_resources where cluster_id = 'migration-test'`).Scan(&count); err != nil || count != 4 {
		t.Errorf("metabase cannot read reporting view: count=%d err=%v", count, err)
	}
	for _, statement := range []string{
		`select 1 from k8s_state.resource_state limit 1`,
		`select 1 from k8s_api.current_resources limit 1`,
		`select 1 from public.github_trust_policies limit 1`,
		`delete from k8s_reporting.current_resources`,
	} {
		assertStatementFails(t, ctx, metabaseURL, statement)
	}
}

func assertApplicationBoundary(t *testing.T, ctx context.Context, applicationURL string) {
	t.Helper()
	connection := connect(t, ctx, applicationURL)
	defer connection.Close(ctx)
	const policyID = "migration-application-boundary"
	if _, err := connection.Exec(ctx, `
		insert into public.github_trust_policies
		(id, owner_sub, name, repository, allowed_namespaces, enabled, created_at, updated_at)
		values ($1, 'owner', 'initial', 'example/repository', array['default'], false, clock_timestamp(), clock_timestamp())`, policyID); err != nil {
		t.Fatalf("application cannot insert github trust policy: %v", err)
	}
	if _, err := connection.Exec(ctx, `update public.github_trust_policies set name = 'updated', updated_at = clock_timestamp() where id = $1`, policyID); err != nil {
		t.Fatalf("application cannot update github trust policy: %v", err)
	}
	var name string
	if err := connection.QueryRow(ctx, `select name from public.github_trust_policies where id = $1`, policyID).Scan(&name); err != nil || name != "updated" {
		t.Fatalf("application cannot read github trust policy: name=%q err=%v", name, err)
	}
	if _, err := connection.Exec(ctx, `delete from public.github_trust_policies where id = $1`, policyID); err != nil {
		t.Fatalf("application cannot delete github trust policy: %v", err)
	}
	for _, statement := range []string{
		`select 1 from k8s_state.resource_state limit 1`,
		`select 1 from k8s_api.current_resources limit 1`,
		`select 1 from k8s_reporting.current_resources limit 1`,
	} {
		assertStatementFails(t, ctx, applicationURL, statement)
	}
}

func connect(t *testing.T, ctx context.Context, databaseURL string) *pgx.Conn {
	t.Helper()
	connection, err := pgx.Connect(ctx, databaseURL)
	if err != nil {
		t.Fatal(err)
	}
	return connection
}

func assertRunFailsBeforeStaticRoleRepair(t *testing.T, ctx context.Context, migrationURL string, credentials CredentialURLs, wantError string) {
	t.Helper()
	err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials})
	if err == nil || !strings.Contains(err.Error(), wantError) {
		t.Fatalf("Run() error = %v, want fail-closed error containing %q", err, wantError)
	}
	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	var inherit bool
	if err := connection.QueryRow(ctx, `select rolinherit from pg_roles where rolname = 'cyclops_app'`).Scan(&inherit); err != nil {
		t.Fatal("read static role after rejected membership drift")
	}
	if !inherit {
		t.Fatal("role reconciliation changed a role before rejecting membership drift")
	}
}

func assertStatementFails(t *testing.T, ctx context.Context, databaseURL, statement string) {
	t.Helper()
	connection := connect(t, ctx, databaseURL)
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, statement); err == nil {
		t.Errorf("statement unexpectedly succeeded: %s", statement)
	}
}

func assertNoPublicSchemaPrivilege(t *testing.T, ctx context.Context, connection *pgx.Conn, schema, privilege string) {
	t.Helper()
	var exists bool
	err := connection.QueryRow(ctx, `select exists(select 1 from pg_namespace namespace join lateral aclexplode(coalesce(namespace.nspacl, acldefault('n', namespace.nspowner))) acl on true where namespace.nspname = $1 and acl.grantee = 0 and acl.privilege_type = $2)`, schema, privilege).Scan(&exists)
	if err != nil || exists {
		t.Errorf("PUBLIC %s on schema %s = %t err=%v", privilege, schema, exists, err)
	}
}

func assertNoPublicTablePrivilege(t *testing.T, ctx context.Context, connection *pgx.Conn, relation string) {
	t.Helper()
	var exists bool
	err := connection.QueryRow(ctx, `select exists(select 1 from pg_class relation join lateral aclexplode(coalesce(relation.relacl, acldefault('r', relation.relowner))) acl on true where relation.oid = $1::regclass and acl.grantee = 0)`, relation).Scan(&exists)
	if err != nil || exists {
		t.Errorf("PUBLIC table privileges on %s = %t err=%v", relation, exists, err)
	}
}

func assertNoPublicFunctionExecute(t *testing.T, ctx context.Context, connection *pgx.Conn, function string) {
	t.Helper()
	var exists bool
	err := connection.QueryRow(ctx, `select exists(select 1 from pg_proc procedure join lateral aclexplode(coalesce(procedure.proacl, acldefault('f', procedure.proowner))) acl on true where procedure.oid = $1::regprocedure and acl.grantee = 0 and acl.privilege_type = 'EXECUTE')`, function).Scan(&exists)
	if err != nil || exists {
		t.Errorf("PUBLIC execute on %s = %t err=%v", function, exists, err)
	}
}

func unregisterTenantRole(t *testing.T, ctx context.Context, roleAdminURL, tenantRole string) {
	t.Helper()
	connection, err := pgx.Connect(ctx, roleAdminURL)
	if err != nil {
		t.Errorf("connect to unregister dynamic role %s: %v", tenantRole, err)
		return
	}
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, `select k8s_state.unregister_tenant_role($1)`, tenantRole); err != nil {
		t.Errorf("unregister dynamic role %s: %v", tenantRole, err)
	}
}

func dropRole(t *testing.T, ctx context.Context, adminURL, role string) {
	t.Helper()
	connection, err := pgx.Connect(ctx, adminURL)
	if err != nil {
		t.Errorf("connect to drop dynamic role %s: %v", role, err)
		return
	}
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, "drop role if exists "+pgx.Identifier{role}.Sanitize()); err != nil {
		t.Errorf("drop dynamic role %s: %v", role, err)
	}
}
