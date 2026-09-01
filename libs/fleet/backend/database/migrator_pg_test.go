package database

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"log/slog"
	"net/url"
	"os"
	"reflect"
	"strings"
	"testing"
	"time"

	"cyclops-cs-backend/chat"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

const migratorIntegrationOptIn = "CYCLOPS_TEST_DATABASE_MIGRATOR_ISOLATED_CLUSTER"

const tenantCredentialFingerprintLookup = `select credential_fingerprint from k8s_state.query_tenant_role where role_name = $1`

var staticMigrationRoles = []string{
	"cyclops_app",
	"cyclops_usage_reader",
	"cyclops_meter_writer",
	"k8s_state_owner",
	"k8s_state_writer",
	"k8s_state_exporter",
	"k8s_query_tenant",
	"k8s_query_admin",
	"k8s_role_admin",
	"k8s_reporting_owner",
	"billing_meter_owner",
	"k8s_metabase",
}

func TestRequireVersionRejectsAbsentLedger(t *testing.T) {
	maintenanceURL := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if maintenanceURL == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run PostgreSQL migration tests")
	}
	if os.Getenv(migratorIntegrationOptIn) != "1" {
		t.Skipf("set %s=1 to run against a dedicated empty PostgreSQL cluster", migratorIntegrationOptIn)
	}

	migrationURL, _ := isolatedMigrationDatabase(t, context.Background(), maintenanceURL)
	err := RequireVersion(context.Background(), migrationURL, 1)
	if err == nil || !strings.Contains(err.Error(), "older than required version 1") {
		t.Fatalf("RequireVersion() error = %v, want missing-ledger version error", err)
	}
}

func TestRequireVersionAllowsRestrictedApplicationRoleAfterMigration(t *testing.T) {
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
	if err := RequireVersion(ctx, credentials.Application, 1); err != nil {
		t.Fatalf("restricted cyclops_app RequireVersion() error = %v", err)
	}
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
	assertInitialMigrationRejectsPublicSecurityDefiner(t, ctx, migrationURL, credentials)
	firstSummary := captureRunSummary(t, func() error {
		return Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials})
	})
	if firstSummary.Pending != 11 || firstSummary.Applied != 11 {
		t.Fatalf("initial migration summary = %+v, want pending=11 applied=11", firstSummary)
	}
	before := migrationLedgerRows(t, ctx, migrationURL)
	secondSummary := captureRunSummary(t, func() error {
		return Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials})
	})
	if secondSummary.Pending != 0 || secondSummary.Applied != 0 {
		t.Fatalf("second migration summary = %+v, want pending=0 applied=0", secondSummary)
	}
	if after := migrationLedgerRows(t, ctx, migrationURL); !reflect.DeepEqual(after, before) {
		t.Fatalf("second migration run changed migration ledger: before=%+v after=%+v", before, after)
	}

	assertRoleContract(t, ctx, migrationURL)
	assertStaticCreatorAdminMemberships(t, ctx, migrationURL)
	bootstrapGrantor := currentRole(t, ctx, maintenanceURL)
	assertImplicitCreatorAdminMembership(t, ctx, migrationURL, "k8s_state_owner", bootstrapGrantor)
	assertImplicitCreatorAdminMembership(t, ctx, migrationURL, "k8s_reporting_owner", bootstrapGrantor)
	assertImplicitCreatorAdminMembership(t, ctx, migrationURL, "billing_meter_owner", bootstrapGrantor)
	assertNoQueryBroker(t, ctx, migrationURL)
	assertOwnershipAndPublicACLs(t, ctx, inspectionURL, currentRole(t, ctx, migrationURL))
	assertSignedServiceURLContract(t, ctx, inspectionURL, credentials.Application, currentRole(t, ctx, migrationURL))
	assertMetabaseBillingMeterAccess(t, ctx, migrationURL, credentials.Metabase)
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
	assertFilteredReservationUsageTenantContract(t, ctx, migrationURL, credentials.Metabase)

	assertWriterBoundary(t, ctx, credentials.Writer)
	assertExporterBoundary(t, ctx, credentials.Exporter)
	assertTenantReadPath(t, ctx, tenantURL)
	assertMetabaseBoundary(t, ctx, inspectionURL, credentials.Metabase)
	assertUsageReaderBoundary(t, ctx, inspectionURL, credentials.Usage)
	assertApplicationBoundary(t, ctx, credentials.Application)
	assertChatConversationStore(t, ctx, credentials.Application)
}

func assertChatConversationStore(t *testing.T, ctx context.Context, applicationURL string) {
	t.Helper()
	first, err := chat.NewPostgresConversationStore(ctx, applicationURL)
	if err != nil {
		t.Fatalf("create first chat store: %v", err)
	}
	defer first.Close()
	second, err := chat.NewPostgresConversationStore(ctx, applicationURL)
	if err != nil {
		t.Fatalf("create second chat store: %v", err)
	}
	defer second.Close()

	conversation, err := first.Create(ctx, "owner-1")
	if err != nil {
		t.Fatalf("create conversation: %v", err)
	}
	if err := first.Append(ctx, "owner-1", conversation.ID, chat.Message{Role: chat.RoleUser, Content: "list pools"}); err != nil {
		t.Fatalf("append through first store: %v", err)
	}
	loaded, err := second.Get(ctx, "owner-1", conversation.ID)
	if err != nil {
		t.Fatalf("load through second store: %v", err)
	}
	if loaded.Title != "list pools" || len(loaded.Messages) != 1 {
		t.Fatalf("second store loaded conversation = %+v", loaded)
	}
	if err := second.Append(ctx, "owner-1", conversation.ID, chat.Message{Role: chat.RoleAssistant, Content: "ready"}); err != nil {
		t.Fatalf("append through second store: %v", err)
	}
	loaded, err = first.Get(ctx, "owner-1", conversation.ID)
	if err != nil {
		t.Fatalf("reload through first store: %v", err)
	}
	if len(loaded.Messages) != 2 || loaded.Messages[1].Content != "ready" {
		t.Fatalf("first store did not observe second store append: %+v", loaded.Messages)
	}
	archived, err := second.SetArchived(ctx, "owner-1", conversation.ID, true)
	if err != nil {
		t.Fatalf("archive through second store: %v", err)
	}
	if archived.ArchivedAt == nil {
		t.Fatal("archived conversation has nil ArchivedAt")
	}
	restored, err := first.SetArchived(ctx, "owner-1", conversation.ID, false)
	if err != nil {
		t.Fatalf("restore through first store: %v", err)
	}
	if restored.ArchivedAt != nil {
		t.Fatalf("restored conversation ArchivedAt = %v, want nil", restored.ArchivedAt)
	}
}

func TestRunUpgradesVersionOneAndThenNoOps(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	applyOnlyMigrationOne(t, ctx, migrationURL)

	upgrade := captureRunSummary(t, func() error {
		return Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials})
	})
	if upgrade.Current != 1 || upgrade.Target != 11 || upgrade.Pending != 10 || upgrade.Applied != 10 || upgrade.Skipped != 1 || upgrade.Result != "success" {
		t.Fatalf("version-one upgrade summary = %+v", upgrade)
	}

	noOp := captureRunSummary(t, func() error {
		return Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials})
	})
	if noOp.Current != 11 || noOp.Target != 11 || noOp.Pending != 0 || noOp.Applied != 0 || noOp.Skipped != 11 || noOp.Result != "success" {
		t.Fatalf("post-upgrade no-op summary = %+v", noOp)
	}
}

func TestRunReconcilesReportingDirectACLDrift(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, `
		create schema reporting_acl_drift;
		create table reporting_acl_drift.probe (id integer primary key);
		create function reporting_acl_drift.probe_function() returns integer language sql as $$ select 1 $$;
		create procedure reporting_acl_drift.probe_procedure() language sql as $$ select 1 $$;
		grant select on reporting_acl_drift.probe to k8s_metabase, k8s_reporting_owner;
		grant execute on function reporting_acl_drift.probe_function() to k8s_metabase, k8s_reporting_owner;
		grant execute on procedure reporting_acl_drift.probe_procedure() to k8s_metabase, k8s_reporting_owner`); err != nil {
		t.Fatal("introduce reporting direct ACL drift")
	}

	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatalf("Run() reconcile reporting direct ACL drift: %v", err)
	}
	assertExactReportingACLContract(t, ctx, connection)
	for _, role := range []string{"k8s_metabase", "k8s_reporting_owner"} {
		var relationACL, routineACL bool
		if err := connection.QueryRow(ctx, `
			select
				exists (
					select 1 from pg_class relation
					join lateral aclexplode(coalesce(relation.relacl, acldefault('r', relation.relowner))) acl on true
					where relation.oid = 'reporting_acl_drift.probe'::regclass
					  and acl.grantee = $1::regrole
				),
				exists (
					select 1 from pg_proc routine
					join lateral aclexplode(coalesce(routine.proacl, acldefault('f', routine.proowner))) acl on true
					where routine.pronamespace = 'reporting_acl_drift'::regnamespace
					  and acl.grantee = $1::regrole
				)`, role).Scan(&relationACL, &routineACL); err != nil {
			t.Fatalf("inspect %s direct ACLs: %v", role, err)
		}
		if relationACL || routineACL {
			t.Errorf("%s retains arbitrary direct ACLs: relation=%t routine=%t", role, relationACL, routineACL)
		}
	}

	var reportingCanReadState bool
	if err := connection.QueryRow(ctx, `
		select has_table_privilege('k8s_reporting_owner'::regrole, relation.oid, 'SELECT')
		from pg_class as relation
		join pg_namespace as namespace on namespace.oid = relation.relnamespace
		where namespace.nspname = 'k8s_state' and relation.relname = 'resource_state'`).Scan(&reportingCanReadState); err != nil || !reportingCanReadState {
		t.Errorf("k8s_reporting_owner state read grant = %t err=%v, want true", reportingCanReadState, err)
	}
}

func TestRunPreservesOwnerDerivedReportingACLsWhileRepairingExplicitDrift(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatalf("initial Run(): %v", err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	before := reportingOwnerACLRows(t, ctx, connection)
	if before == "" {
		t.Fatal("reporting fixtures must expose owner-derived ACL rows")
	}
	if _, err := connection.Exec(ctx, `
		set role k8s_reporting_owner;
		grant insert on k8s_reporting.current_resources to k8s_metabase;
		reset role`); err != nil {
		t.Fatal("introduce explicit reporting view write drift")
	}
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatalf("Run() repair explicit reporting view drift: %v", err)
	}
	if after := reportingOwnerACLRows(t, ctx, connection); after != before {
		t.Fatalf("owner-derived reporting ACL rows changed: before=%q after=%q", before, after)
	}
	assertExactReportingACLContract(t, ctx, connection)
}

func TestRunRepairsExactReportingACLContract(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	t.Cleanup(func() { dropRole(t, context.Background(), maintenanceURL, "reporting_acl_unexpected") })
	if _, err := connection.Exec(ctx, `
		create schema reporting_acl_drift;
		create table reporting_acl_drift.probe (id integer primary key);
		create sequence reporting_acl_drift.probe_sequence;
		create function reporting_acl_drift.probe_function() returns integer language sql as $$ select 1 $$;
		create schema "reporting ACL drift";
		create table "reporting ACL drift"."probe.table" (id integer primary key);
		create sequence "reporting ACL drift"."probe.sequence";
		create function "reporting ACL drift"."probe.function"(integer) returns integer language sql as $$ select $1 $$;
		set role k8s_state_owner;
		grant usage, create on schema k8s_state to k8s_metabase;
		reset role;
		grant create on schema cyclops_migrations to k8s_metabase;
		grant insert, update, delete on cyclops_migrations.applied_migrations to k8s_metabase;
		set role k8s_reporting_owner;
		grant insert, update, delete on k8s_reporting.current_resources to k8s_metabase;
		grant usage, create on schema k8s_reporting to public;
		reset role;
		grant select on reporting_acl_drift.probe to k8s_metabase, k8s_reporting_owner;
		grant usage on sequence reporting_acl_drift.probe_sequence to k8s_metabase, k8s_reporting_owner;
		grant execute on function reporting_acl_drift.probe_function() to k8s_metabase, k8s_reporting_owner;
		grant usage on schema "reporting ACL drift" to k8s_metabase, k8s_reporting_owner;
		grant select on "reporting ACL drift"."probe.table" to k8s_metabase, k8s_reporting_owner;
		grant usage on sequence "reporting ACL drift"."probe.sequence" to k8s_metabase, k8s_reporting_owner;
		grant execute on function "reporting ACL drift"."probe.function"(integer) to k8s_metabase, k8s_reporting_owner;
		create role reporting_acl_unexpected nologin;
		set role k8s_reporting_owner;
		grant usage on schema k8s_reporting to reporting_acl_unexpected;
		grant select on k8s_reporting.current_resources to reporting_acl_unexpected;
		reset role`); err != nil {
		t.Fatalf("introduce recoverable reporting ACL drift: %v", err)
	}

	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatalf("Run() reconcile exact reporting ACL contract: %v", err)
	}
	assertExactReportingACLContract(t, ctx, connection)
}

func TestRunFailsClosedBeforeReportingACLMutation(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, `
		create schema reporting_acl_sentinel;
		create table reporting_acl_sentinel.probe (id integer primary key);
		grant select on reporting_acl_sentinel.probe to k8s_metabase;
		set role k8s_reporting_owner;
		drop view k8s_reporting.current_resources;
		create table k8s_reporting.current_resources (id integer);
		reset role`); err != nil {
		t.Fatalf("introduce incompatible reporting relation and ACL sentinel: %v", err)
	}

	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err == nil || !strings.Contains(err.Error(), "reporting relation") {
		t.Fatalf("Run() error = %v, want reporting relation fail-closed error", err)
	}
	var retained bool
	if err := connection.QueryRow(ctx, `select has_table_privilege('k8s_metabase', 'reporting_acl_sentinel.probe', 'SELECT')`).Scan(&retained); err != nil || !retained {
		t.Fatalf("unrelated ACL sentinel changed after rejected reporting drift: retained=%t err=%v", retained, err)
	}
}

func TestRunFailsClosedForExternalReportingACLGrantor(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	t.Cleanup(func() {
		cleanup := connect(t, context.Background(), migrationURL)
		defer cleanup.Close(context.Background())
		if _, err := cleanup.Exec(context.Background(), `set role reporting_acl_external_owner; drop schema reporting_acl_external cascade; reset role`); err != nil {
			t.Errorf("drop externally owned reporting ACL fixture: %v", err)
		}
		dropRole(t, context.Background(), maintenanceURL, "reporting_acl_external_owner")
	})
	if _, err := connection.Exec(ctx, `
		create role reporting_acl_external_owner nologin;
		grant reporting_acl_external_owner to current_user with inherit false, set true;
		create schema reporting_acl_external;
		alter schema reporting_acl_external owner to reporting_acl_external_owner;
		set role reporting_acl_external_owner;
		create table reporting_acl_external.probe (id integer primary key);
		create function reporting_acl_external.probe_function() returns integer language sql as $$ select 1 $$;
		grant usage on schema reporting_acl_external to k8s_metabase;
		grant select on reporting_acl_external.probe to k8s_reporting_owner;
		grant execute on function reporting_acl_external.probe_function() to k8s_metabase;
		reset role`); err != nil {
		t.Fatalf("introduce externally owned reporting ACL drift: %v", err)
	}

	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err == nil || !strings.Contains(err.Error(), "unsafe reporting ACL") {
		t.Fatalf("Run() error = %v, want unsafe reporting ACL fail-closed error", err)
	}
	for _, statement := range []string{
		`select has_schema_privilege('k8s_metabase', namespace.oid, 'USAGE') from pg_namespace as namespace where namespace.nspname = 'reporting_acl_external'`,
		`select has_table_privilege('k8s_reporting_owner', relation.oid, 'SELECT') from pg_class as relation join pg_namespace as namespace on namespace.oid = relation.relnamespace where namespace.nspname = 'reporting_acl_external' and relation.relname = 'probe'`,
		`select has_function_privilege('k8s_metabase', routine.oid, 'EXECUTE') from pg_proc as routine join pg_namespace as namespace on namespace.oid = routine.pronamespace where namespace.nspname = 'reporting_acl_external' and routine.proname = 'probe_function'`,
	} {
		var retained bool
		if err := connection.QueryRow(ctx, statement).Scan(&retained); err != nil || !retained {
			t.Fatalf("external ACL changed after rejected drift for %q: retained=%t err=%v", statement, retained, err)
		}
	}
}

func TestRunRechecksPublicSecurityDefinerBeforeMutations(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, `create function public.repeat_run_public_definer() returns integer language sql security definer as $$ select 1 $$`); err != nil {
		t.Fatal("create public security-definer fixture")
	}
	var before string
	if err := connection.QueryRow(ctx, `select coalesce(proacl::text, '<default>') from pg_proc where oid = 'public.repeat_run_public_definer()'::regprocedure`).Scan(&before); err != nil {
		t.Fatal("read public security-definer ACL before Run")
	}
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err == nil || !strings.Contains(err.Error(), "PUBLIC-executable SECURITY DEFINER routine") {
		t.Fatalf("Run() error = %v, want PUBLIC-executable SECURITY DEFINER routine", err)
	}
	var after string
	if err := connection.QueryRow(ctx, `select coalesce(proacl::text, '<default>') from pg_proc where oid = 'public.repeat_run_public_definer()'::regprocedure`).Scan(&after); err != nil {
		t.Fatal("read public security-definer ACL after Run")
	}
	if after != before {
		t.Fatalf("Run() changed blocked security-definer routine ACL: before=%q after=%q", before, after)
	}
}

func TestRunReconcilesPublicExecuteOnExpectedUsageRoutine(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, `set role k8s_reporting_owner; grant execute on function k8s_reporting.usage_sandbox_events(text, timestamptz, timestamptz) to public; reset role`); err != nil {
		t.Fatal("introduce PUBLIC usage routine execute drift")
	}

	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatalf("Run() reconcile PUBLIC usage routine execute drift: %v", err)
	}
	inspection := connect(t, ctx, maintenanceURLForDatabase(t, maintenanceURL, migrationURL))
	defer inspection.Close(ctx)
	assertNoPublicFunctionExecute(t, ctx, inspection, "k8s_reporting.usage_sandbox_events(text,timestamptz,timestamptz)")
	assertExactReportingACLContract(t, ctx, inspection)
}

func TestRunDeniesMetabaseProcedureWriteAfterReadOnlyGUCBypass(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, `
		set role k8s_reporting_owner;
		create procedure k8s_reporting.write_probe() language plpgsql as $$
		begin
			insert into k8s_state.resource_state (
				cluster_id, api_group, resource, namespace, name, schema_hash, watch_epoch, observed_sequence, labels, object
			) values ('metabase-write-probe', '', 'pods', 'default', 'probe', 'probe', 1, 1, '{}'::jsonb, '{}'::jsonb);
		end
		$$;
		grant execute on procedure k8s_reporting.write_probe() to k8s_metabase;
		reset role`); err != nil {
		t.Fatal("create executable metabase write probe")
	}

	metabase := connect(t, ctx, credentials.Metabase)
	defer metabase.Close(ctx)
	if _, err := metabase.Exec(ctx, `set default_transaction_read_only = off`); err != nil {
		t.Fatal("disable metabase read-only default")
	}
	if _, err := metabase.Exec(ctx, `call k8s_reporting.write_probe()`); err == nil || !strings.Contains(strings.ToLower(err.Error()), "permission denied") {
		t.Fatalf("metabase procedure write error = %v, want privilege denial", err)
	}
}

func TestMetabaseReadOnlyDefaultBlocksExecutableWriteProcedure(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, `
		set role k8s_reporting_owner;
		create table k8s_reporting.read_only_probe (id integer primary key);
		create procedure k8s_reporting.read_only_write_probe() language sql as $$ insert into k8s_reporting.read_only_probe values (1) $$;
		grant execute on procedure k8s_reporting.read_only_write_probe() to k8s_metabase;
		reset role`); err != nil {
		t.Fatal("create metabase read-only procedure fixture")
	}

	metabase := connect(t, ctx, credentials.Metabase)
	defer metabase.Close(ctx)
	if _, err := metabase.Exec(ctx, `call k8s_reporting.read_only_write_probe()`); err == nil || !strings.Contains(strings.ToLower(err.Error()), "read-only") {
		t.Fatalf("metabase read-only procedure error = %v, want read-only failure", err)
	}
}

func TestRunFailsClosedForReportingObjectDrift(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()

	for _, testCase := range []struct {
		name            string
		introduce       func(t *testing.T, connection *pgx.Conn)
		assertUnchanged func(t *testing.T, connection *pgx.Conn)
		wantError       string
	}{
		{
			name: "relation type",
			introduce: func(t *testing.T, connection *pgx.Conn) {
				t.Helper()
				if _, err := connection.Exec(ctx, `drop view k8s_reporting.current_resources; create table k8s_reporting.current_resources (id integer)`); err != nil {
					t.Fatal("replace reporting view with table")
				}
			},
			assertUnchanged: func(t *testing.T, connection *pgx.Conn) {
				t.Helper()
				var relationKind string
				if err := connection.QueryRow(ctx, `select relkind::text from pg_class where oid = 'k8s_reporting.current_resources'::regclass`).Scan(&relationKind); err != nil || relationKind != "r" {
					t.Fatalf("reporting relation kind after rejected drift = %q err=%v, want r", relationKind, err)
				}
			},
			wantError: "reporting relation",
		},

		{
			name: "view owner",
			introduce: func(t *testing.T, connection *pgx.Conn) {
				t.Helper()
				if _, err := connection.Exec(ctx, `alter view k8s_reporting.current_resources owner to current_user`); err != nil {
					t.Fatal("change reporting view owner")
				}
			},
			assertUnchanged: func(t *testing.T, connection *pgx.Conn) {
				t.Helper()
				var owner string
				if err := connection.QueryRow(ctx, `select relowner::regrole::text from pg_class where oid = 'k8s_reporting.current_resources'::regclass`).Scan(&owner); err != nil || owner == "k8s_reporting_owner" {
					t.Fatalf("reporting view owner after rejected drift = %q err=%v, want non-reporting owner", owner, err)
				}
			},
			wantError: "reporting view owner",
		},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
			if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
				t.Fatal(err)
			}
			inspectionURL := maintenanceURLForDatabase(t, maintenanceURL, migrationURL)
			connection := connect(t, ctx, inspectionURL)
			defer connection.Close(ctx)
			testCase.introduce(t, connection)
			if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err == nil || !strings.Contains(err.Error(), testCase.wantError) {
				t.Fatalf("Run() error = %v, want fail-closed error containing %q", err, testCase.wantError)
			}
			testCase.assertUnchanged(t, connection)
		})
	}
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

func TestRunReconcilesStaticRoleCreateDBDrift(t *testing.T) {
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
	migrationOwner := currentRole(t, ctx, migrationURL)
	if _, err := maintenance.Exec(ctx, "alter role "+pgx.Identifier{migrationOwner}.Sanitize()+` createdb; alter role cyclops_app nologin createdb`); err != nil {
		t.Fatal("introduce static role createdb drift")
	}

	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatalf("Run() reconcile static role createdb drift: %v", err)
	}

	var login, createDB bool
	if err := maintenance.QueryRow(ctx, `select rolcanlogin, rolcreatedb from pg_roles where rolname = 'cyclops_app'`).Scan(&login, &createDB); err != nil {
		t.Fatal("read static role after rejected createdb drift")
	}
	if !login || createDB {
		t.Fatalf("static role after createdb reconciliation: login:%t createdb:%t", login, createDB)
	}
}

func TestRunFailsClosedForStaticRoleCreateDBDriftWithoutAuthority(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}
	maintenance := connect(t, ctx, maintenanceURL)
	defer maintenance.Close(ctx)
	if _, err := maintenance.Exec(ctx, `alter role cyclops_app nologin createdb`); err != nil {
		t.Fatal("introduce static role createdb drift without migrator authority")
	}

	err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials})
	if err == nil || !strings.Contains(err.Error(), "permission denied to alter role") {
		t.Fatalf("Run() error = %v, want fail-closed role authority error", err)
	}

	var login, createDB bool
	if err := maintenance.QueryRow(ctx, `select rolcanlogin, rolcreatedb from pg_roles where rolname = 'cyclops_app'`).Scan(&login, &createDB); err != nil {
		t.Fatal("read static role after unauthorized createdb reconciliation")
	}
	if login || !createDB {
		t.Fatalf("static role changed after unauthorized createdb reconciliation: login:%t createdb:%t", login, createDB)
	}
}

func TestValidateStaticRoleContractsAllowsMissingCreatorAdminMembership(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	migrationOwner := currentRole(t, ctx, migrationURL)
	maintenance := connect(t, ctx, maintenanceURLForDatabase(t, maintenanceURL, migrationURL))
	defer maintenance.Close(ctx)
	migrationOwnerIdentifier := pgx.Identifier{migrationOwner}.Sanitize()
	maintenanceRole := currentRole(t, ctx, maintenanceURL)
	maintenanceIdentifier := pgx.Identifier{maintenanceRole}.Sanitize()
	const role = "cyclops_app"
	if _, err := maintenance.Exec(ctx, "revoke "+pgx.Identifier{role}.Sanitize()+" from "+migrationOwnerIdentifier+" granted by "+maintenanceIdentifier+" restrict"); err != nil {
		t.Fatalf("remove creator-admin membership for %s: %v", role, err)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	transaction, err := connection.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer transaction.Rollback(ctx)
	if err := validateStaticRoleMemberships(ctx, transaction, migrationOwner, staticMembershipContracts(migrationOwner)); err != nil {
		t.Fatal(err)
	}
	if rows := staticMembershipRows(t, ctx, maintenance, role, migrationOwner); len(rows) != 0 {
		t.Fatalf("role %s creator memberships = %+v, want none", role, rows)
	}
}

func TestRunPreservesMaintenanceSuperuserStaticMembershipGrant(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}

	migrationOwner := currentRole(t, ctx, migrationURL)
	maintenance := connect(t, ctx, maintenanceURLForDatabase(t, maintenanceURL, migrationURL))
	defer maintenance.Close(ctx)
	migrationOwnerIdentifier := pgx.Identifier{migrationOwner}.Sanitize()
	if _, err := maintenance.Exec(ctx, "revoke k8s_query_tenant from k8s_role_admin granted by "+migrationOwnerIdentifier+" restrict"); err != nil {
		t.Fatal("remove migration-owner static membership")
	}
	if _, err := maintenance.Exec(ctx, `grant k8s_query_tenant to k8s_role_admin with admin true, inherit false, set false`); err != nil {
		t.Fatal("create maintenance-superuser static membership")
	}

	before := staticMembershipRows(t, ctx, maintenance, "k8s_query_tenant", "k8s_role_admin")
	if len(before) != 1 || before[0].grantor == migrationOwner || !before[0].grantorSuperuser || !before[0].admin || before[0].inherit || before[0].set {
		t.Fatalf("maintenance-superuser static membership = %+v, want one exact grant from a superuser other than %s", before, migrationOwner)
	}

	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
		t.Fatal(err)
	}
	if after := staticMembershipRows(t, ctx, maintenance, "k8s_query_tenant", "k8s_role_admin"); !reflect.DeepEqual(after, before) {
		t.Fatalf("maintenance-superuser static membership changed during rerun: before=%+v after=%+v", before, after)
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

func TestRunPreflightLeavesEarlierRepairableStaticMembershipUnchanged(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)

	ctx := context.Background()

	for _, testCase := range []struct {
		name           string
		introduceFirst func(t *testing.T, maintenance *pgx.Conn, migrationOwner string)
		introduceLater func(t *testing.T, maintenance *pgx.Conn, migrationURL, migrationOwner string)
		wantError      string
	}{
		{
			name: "missing before foreign membership",
			introduceFirst: func(t *testing.T, maintenance *pgx.Conn, migrationOwner string) {
				t.Helper()
				identifier := pgx.Identifier{migrationOwner}.Sanitize()
				if _, err := maintenance.Exec(ctx, "revoke k8s_state_owner from "+identifier+" granted by "+identifier+" restrict"); err != nil {
					t.Fatal("remove repairable state-owner membership")
				}
			},
			introduceLater: func(t *testing.T, maintenance *pgx.Conn, migrationURL, migrationOwner string) {
				t.Helper()
				foreignGrantor := "foreign_grantor_" + testToken(t)[:12]
				foreignIdentifier := pgx.Identifier{foreignGrantor}.Sanitize()
				t.Cleanup(func() {
					cleanupCtx := context.Background()
					connection := connect(t, cleanupCtx, maintenanceURLForDatabase(t, maintenanceURL, migrationURL))
					defer connection.Close(cleanupCtx)
					if _, err := connection.Exec(cleanupCtx, "drop owned by "+foreignIdentifier); err != nil {
						t.Errorf("drop foreign grantor-owned objects: %v", err)
					}
					dropRole(t, cleanupCtx, maintenanceURL, foreignGrantor)
				})
				if _, err := maintenance.Exec(ctx, "create role "+foreignIdentifier); err != nil {
					t.Fatal("create foreign grantor")
				}
				if _, err := maintenance.Exec(ctx, "grant k8s_query_tenant to "+foreignIdentifier+" with admin true, inherit false, set false"); err != nil {
					t.Fatal("grant query tenant admin to foreign grantor")
				}
				identifier := pgx.Identifier{migrationOwner}.Sanitize()
				if _, err := maintenance.Exec(ctx, "revoke k8s_query_tenant from k8s_role_admin granted by "+identifier+" restrict"); err != nil {
					t.Fatal("remove migration-owner query tenant membership")
				}
				if _, err := maintenance.Exec(ctx, "set role "+foreignIdentifier); err != nil {
					t.Fatal("set foreign grantor role")
				}
				if _, err := maintenance.Exec(ctx, `grant k8s_query_tenant to k8s_role_admin with admin true, inherit false, set false`); err != nil {
					t.Fatal("create foreign static membership")
				}
				if _, err := maintenance.Exec(ctx, `reset role`); err != nil {
					t.Fatal("reset foreign grantor role")
				}
			},
			wantError: "has grantor foreign_grantor_",
		},
		{
			name: "SET drift before duplicate membership",
			introduceFirst: func(t *testing.T, maintenance *pgx.Conn, migrationOwner string) {
				t.Helper()
				identifier := pgx.Identifier{migrationOwner}.Sanitize()
				if _, err := maintenance.Exec(ctx, "revoke k8s_state_owner from "+identifier+" granted by "+identifier+" restrict"); err != nil {
					t.Fatal("remove state-owner membership before SET drift")
				}
				if _, err := maintenance.Exec(ctx, "grant k8s_state_owner to "+identifier+" with admin false, inherit false, set false granted by "+identifier); err != nil {
					t.Fatal("introduce repairable state-owner SET drift")
				}
			},
			introduceLater: func(t *testing.T, maintenance *pgx.Conn, migrationURL, migrationOwner string) {
				t.Helper()
				if _, err := maintenance.Exec(ctx, `grant k8s_query_tenant to k8s_role_admin with admin true, inherit false, set false`); err != nil {
					t.Fatal("create postgres-grantor duplicate static membership")
				}
			},
			wantError: "has 2 non-implicit grants",
		},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
			if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err != nil {
				t.Fatal("initial Run()", err)
			}
			migrationOwner := currentRole(t, ctx, migrationURL)
			maintenance := connect(t, ctx, maintenanceURL)
			defer maintenance.Close(ctx)
			testCase.introduceFirst(t, maintenance, migrationOwner)
			testCase.introduceLater(t, maintenance, migrationURL, migrationOwner)

			connection := connect(t, ctx, migrationURL)
			defer connection.Close(ctx)
			before := staticMembershipRows(t, ctx, connection, "k8s_state_owner", migrationOwner)
			err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials})
			if err == nil || !strings.Contains(err.Error(), testCase.wantError) {
				t.Fatalf("Run() error = %v, want fail-closed error containing %q", err, testCase.wantError)
			}
			if after := staticMembershipRows(t, ctx, connection, "k8s_state_owner", migrationOwner); !reflect.DeepEqual(after, before) {
				t.Fatalf("repairable state-owner membership changed after fail-closed preflight: before=%+v after=%+v", before, after)
			}
		})
	}
}

func staticMembershipRows(t *testing.T, ctx context.Context, connection *pgx.Conn, role, member string) []staticMembershipGrant {
	t.Helper()
	rows, err := connection.Query(ctx, `
		select grantor_role.rolname, grantor_role.rolsuper, membership.admin_option, membership.inherit_option, membership.set_option
		from pg_auth_members as membership
		join pg_roles as grantor_role on grantor_role.oid = membership.grantor
		where membership.roleid = $1::regrole and membership.member = $2::regrole
		order by grantor_role.rolname`, role, member)
	if err != nil {
		t.Fatalf("read static membership %s -> %s: %v", role, member, err)
	}
	defer rows.Close()

	var grants []staticMembershipGrant
	for rows.Next() {
		var grant staticMembershipGrant
		if err := rows.Scan(&grant.grantor, &grant.grantorSuperuser, &grant.admin, &grant.inherit, &grant.set); err != nil {
			t.Fatalf("scan static membership %s -> %s: %v", role, member, err)
		}
		grants = append(grants, grant)
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("iterate static membership %s -> %s: %v", role, member, err)
	}
	return grants
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

	assertRunFailsBeforeStaticRoleRepair(t, ctx, migrationURL, credentials, "has grantor")
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
		`alter role cyclops_usage_reader set statement_timeout = '1ms'`,
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
	assertStaticRoleSettings(t, ctx, connection, "cyclops_usage_reader", map[string]string{
		"default_transaction_read_only":       "on",
		"statement_timeout":                   "10000ms",
		"idle_in_transaction_session_timeout": "10000ms",
	})
	for _, role := range staticMigrationRoles {
		if role == "k8s_metabase" || role == "cyclops_usage_reader" {
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

func applyOnlyMigrationOne(t *testing.T, ctx context.Context, migrationURL string) {
	t.Helper()
	files, err := embeddedMigrations()
	if err != nil {
		t.Fatal(err)
	}
	if len(files) < 2 || files[0].Version != 1 {
		t.Fatalf("embedded migrations = %+v, want immutable version one followed by current migrations", files)
	}

	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	transaction, err := connection.Begin(ctx)
	if err != nil {
		t.Fatal("begin version-one migration fixture")
	}
	defer transaction.Rollback(ctx)
	if err := newRuntimeDDL(transaction).ensureMigrationLedger(ctx); err != nil {
		t.Fatalf("prepare version-one migration ledger: %v", err)
	}
	if _, err := transaction.Exec(ctx, files[0].SQL); err != nil {
		t.Fatalf("apply immutable migration one: %v", err)
	}
	if _, err := transaction.Exec(ctx, insertAppliedMigrationStatement, files[0].Version, files[0].Name, files[0].SHA256); err != nil {
		t.Fatalf("record immutable migration one: %v", err)
	}
	if err := transaction.Commit(ctx); err != nil {
		t.Fatalf("commit immutable migration one: %v", err)
	}
}

func captureRunSummary(t *testing.T, run func() error) migrationSummary {
	t.Helper()
	var output bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewJSONHandler(&output, nil)))
	defer slog.SetDefault(previous)

	if err := run(); err != nil {
		t.Fatal(err)
	}
	for _, line := range strings.Split(strings.TrimSpace(output.String()), "\n") {
		var entry struct {
			Message string `json:"msg"`
			Current int64  `json:"current_version"`
			Target  int64  `json:"target_version"`
			Pending int    `json:"pending"`
			Applied int    `json:"applied"`
			Skipped int    `json:"skipped"`
			Result  string `json:"result"`
		}
		if err := json.Unmarshal([]byte(line), &entry); err != nil {
			t.Fatalf("decode migration log entry: %v", err)
		}
		if entry.Message == "database migration summary" {
			return migrationSummary{
				Current: entry.Current,
				Target:  entry.Target,
				Pending: entry.Pending,
				Applied: entry.Applied,
				Skipped: entry.Skipped,
				Result:  entry.Result,
			}
		}
	}
	t.Fatalf("migration summary missing from logs: %s", output.String())
	return migrationSummary{}
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
		Usage:       withRole("cyclops_usage_reader"),
		Meter:       withRole("cyclops_meter_writer"),
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
	if len(ledger) != 11 {
		t.Fatalf("migration ledger row count = %d, want 11", len(ledger))
	}
	for index, want := range []struct {
		version  int64
		filename string
	}{
		{1, "000001_initial_schema.sql"},
		{2, "000002_usage_sandbox_events.sql"},
		{3, "000003_usage_claimed_sandbox_pool.sql"},
		{4, "000004_filter_invalid_usage_sandbox_events.sql"},
		{5, "000005_hourly_reservation_meter.sql"},
		{6, "000006_chat_conversations.sql"},
		{7, "000007_metabase_hourly_reservation_usage.sql"},
		{8, "000008_metabase_hourly_reservation_usage_excluding_tenants.sql"},
		{9, "000009_extend_metabase_revenue_tenant_exclusions.sql"},
		{10, "000010_grant_metabase_billing_meter_access.sql"},
		{11, "000011_signed_service_urls.sql"},
	} {
		if ledger[index].Version != want.version || ledger[index].ApplicationOrder != int64(index+1) || ledger[index].Filename != want.filename {
			t.Fatalf("migration ledger row %d = version:%d order:%d filename:%q", index, ledger[index].Version, ledger[index].ApplicationOrder, ledger[index].Filename)
		}
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
		allowOwnerGrant := role == "k8s_state_owner" || role == "k8s_reporting_owner" || role == "billing_meter_owner"
		if !staticCreatorAdminMembershipsAreExact(migrationOwner, grants, allowOwnerGrant, nil) {
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
		"cyclops_app":          {true, false, false},
		"k8s_state_owner":      {false, true, false},
		"k8s_state_writer":     {true, false, false},
		"k8s_state_exporter":   {true, false, false},
		"k8s_query_tenant":     {false, true, false},
		"k8s_query_admin":      {false, true, false},
		"k8s_role_admin":       {true, false, true},
		"k8s_reporting_owner":  {false, true, false},
		"billing_meter_owner":  {false, true, false},
		"k8s_metabase":         {true, false, false},
		"cyclops_usage_reader": {true, false, false},
		"cyclops_meter_writer": {true, false, false},
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
		{"public", "chat_conversations", migrationOwner},
		{"public", "signed_service_urls", migrationOwner},
		{"k8s_state", "resource_state", "k8s_state_owner"},
		{"k8s_state", "resource_event_outbox", "k8s_state_owner"},
		{"k8s_state", "resource_event_outbox_usage_lookup_idx", "k8s_state_owner"},
		{"k8s_api", "current_resources", "k8s_state_owner"},
		{"k8s_reporting", "current_resources", "k8s_reporting_owner"},
		{"k8s_reporting", "hourly_reservation_usage", "k8s_reporting_owner"},
		{"k8s_reporting", "hourly_reservation_usage_excluding_tenants", "k8s_reporting_owner"},
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
	for _, relation := range []string{"public.github_trust_policies", "public.chat_conversations", "public.signed_service_urls", "k8s_state.resource_state", "k8s_state.resource_event_outbox", "k8s_api.current_resources", "k8s_reporting.current_resources", "k8s_reporting.hourly_reservation_usage", "k8s_reporting.hourly_reservation_usage_excluding_tenants", "cyclops_migrations.applied_migrations"} {
		assertNoPublicTablePrivilege(t, ctx, connection, relation)
	}
}

func assertSignedServiceURLContract(t *testing.T, ctx context.Context, inspectionURL, applicationURL, migrationOwner string) {
	t.Helper()
	inspection := connect(t, ctx, inspectionURL)
	defer inspection.Close(ctx)
	assertRelationOwner(t, inspection, "public", "signed_service_urls", migrationOwner)
	assertTablePrivileges(t, inspection, "cyclops_app", "public", "signed_service_urls", []string{"SELECT", "INSERT", "UPDATE"})

	application := connect(t, ctx, applicationURL)
	defer application.Close(ctx)
	createdAt := time.Date(2026, time.August, 31, 0, 0, 0, 0, time.UTC)
	insert := func(id string, expiresAt time.Time) error {
		_, err := application.Exec(ctx, `
			insert into public.signed_service_urls
				(id, namespace, claim_name, sandbox_name, service_name, logical_service, label, creator_sub, created_at, expires_at)
			values ($1, 'tenant-a', 'claim-a', 'sandbox-a', 'service-a', 'desktop', 'Desktop', 'user-a', $2, $3)`,
			id, createdAt, expiresAt)
		return err
	}
	if err := insert("00000000-0000-0000-0000-000000000001", createdAt.Add(time.Minute)); err != nil {
		t.Fatalf("insert one-minute signed URL: %v", err)
	}
	if err := insert("00000000-0000-0000-0000-000000000002", createdAt.Add(24*time.Hour)); err != nil {
		t.Fatalf("insert 24-hour signed URL: %v", err)
	}
	assertCheckViolation(t, insert("00000000-0000-0000-0000-000000000003", createdAt.Add(59*time.Second)))
	assertCheckViolation(t, insert("00000000-0000-0000-0000-000000000004", createdAt.Add(24*time.Hour+time.Second)))

	var count int
	if err := application.QueryRow(ctx, `select count(*) from public.signed_service_urls`).Scan(&count); err != nil || count != 2 {
		t.Fatalf("signed URL row count = %d, err=%v, want 2", count, err)
	}
	if _, err := application.Exec(ctx, `update public.signed_service_urls set revoked_at = $1 where id = $2`, createdAt, "00000000-0000-0000-0000-000000000001"); err != nil {
		t.Fatalf("revoke signed URL: %v", err)
	}
}

func assertRelationOwner(t *testing.T, connection *pgx.Conn, schema, relation, wantOwner string) {
	t.Helper()
	var owner string
	if err := connection.QueryRow(context.Background(), `
		select relation.relowner::regrole::text
		from pg_class relation
		join pg_namespace namespace on namespace.oid = relation.relnamespace
		where namespace.nspname = $1 and relation.relname = $2`, schema, relation).Scan(&owner); err != nil {
		t.Fatalf("read owner for %s.%s: %v", schema, relation, err)
	}
	if owner != wantOwner {
		t.Fatalf("owner for %s.%s = %s, want %s", schema, relation, owner, wantOwner)
	}
}

func assertTablePrivileges(t *testing.T, connection *pgx.Conn, role, schema, table string, want []string) {
	t.Helper()
	qualified := pgx.Identifier{schema, table}.Sanitize()
	var got []string
	for _, privilege := range []string{"SELECT", "INSERT", "UPDATE", "DELETE", "TRUNCATE", "REFERENCES", "TRIGGER"} {
		var allowed bool
		if err := connection.QueryRow(context.Background(), `select has_table_privilege($1, $2, $3)`, role, qualified, privilege).Scan(&allowed); err != nil {
			t.Fatalf("read %s privilege for %s on %s: %v", privilege, role, qualified, err)
		}
		if allowed {
			got = append(got, privilege)
		}
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("table privileges for %s on %s = %v, want %v", role, qualified, got, want)
	}
}

func assertCheckViolation(t *testing.T, err error) {
	t.Helper()
	var pgError *pgconn.PgError
	if !errors.As(err, &pgError) || pgError.Code != "23514" {
		t.Fatalf("insert error = %v, want PostgreSQL check violation", err)
	}
}

func assertMetabaseBillingMeterAccess(t *testing.T, ctx context.Context, adminURL, metabaseURL string) {
	t.Helper()
	admin := connect(t, ctx, adminURL)
	defer admin.Close(ctx)

	rows, err := admin.Query(ctx, `
		select relation.relname
		from pg_class relation
		join pg_namespace namespace on namespace.oid = relation.relnamespace
		where namespace.nspname = 'billing_meter'
		  and relation.relkind in ('r', 'p', 'v', 'm', 'f')
		order by relation.relname`)
	if err != nil {
		t.Fatal("list billing meter relations")
	}
	var relations []string
	for rows.Next() {
		var relation string
		if err := rows.Scan(&relation); err != nil {
			rows.Close()
			t.Fatal("scan billing meter relation")
		}
		relations = append(relations, relation)
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		t.Fatalf("iterate billing meter relations: %v", err)
	}
	rows.Close()
	if len(relations) == 0 {
		t.Fatal("expected billing meter relations")
	}

	metabase := connect(t, ctx, metabaseURL)
	defer metabase.Close(ctx)
	for _, relation := range relations {
		query := "select 1 from " + pgx.Identifier{"billing_meter", relation}.Sanitize() + " limit 1"
		if _, err := metabase.Exec(ctx, query); err != nil {
			t.Errorf("Metabase cannot select billing_meter.%s: %v", relation, err)
		}
	}

	const futureRelation = "metabase_default_privilege_probe"
	if _, err := admin.Exec(ctx, `
		set role billing_meter_owner;
		drop table if exists billing_meter.metabase_default_privilege_probe;
		create table billing_meter.metabase_default_privilege_probe (id integer);
		reset role`); err != nil {
		t.Fatal("create billing meter default privilege probe")
	}
	query := "select 1 from " + pgx.Identifier{"billing_meter", futureRelation}.Sanitize() + " limit 1"
	if _, err := metabase.Exec(ctx, query); err != nil {
		t.Fatalf("Metabase cannot select future billing meter table: %v", err)
	}
	if _, err := admin.Exec(ctx, `set role billing_meter_owner; drop table billing_meter.metabase_default_privilege_probe; reset role`); err != nil {
		t.Fatalf("drop billing meter default privilege probe: %v", err)
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
	} {
		connection := connect(t, ctx, databaseURL)
		var count int
		err := connection.QueryRow(ctx, `select count(*) from cyclops_migrations.applied_migrations`).Scan(&count)
		connection.Close(ctx)
		if err != nil || count != 11 {
			t.Errorf("%s ledger select = count:%d err:%v", role, count, err)
		}
		assertStatementFails(t, ctx, databaseURL, `insert into cyclops_migrations.applied_migrations (version, filename, sha256) values (99, 'invalid.sql', 'invalid')`)
		assertStatementFails(t, ctx, databaseURL, `update cyclops_migrations.applied_migrations set filename = 'invalid.sql' where version = 1`)
		assertStatementFails(t, ctx, databaseURL, `delete from cyclops_migrations.applied_migrations where version = 1`)
	}
	assertStatementFails(t, ctx, credentials.Metabase, `select 1 from cyclops_migrations.applied_migrations limit 1`)
	assertStatementFails(t, ctx, credentials.Metabase, `insert into cyclops_migrations.applied_migrations (version, filename, sha256) values (99, 'invalid.sql', 'invalid')`)
	assertStatementFails(t, ctx, credentials.Metabase, `update cyclops_migrations.applied_migrations set filename = 'invalid.sql' where version = 1`)
	assertStatementFails(t, ctx, credentials.Metabase, `delete from cyclops_migrations.applied_migrations where version = 1`)
	assertStatementFails(t, ctx, credentials.Usage, `select 1 from cyclops_migrations.applied_migrations limit 1`)
}

func assertUsageReaderBoundary(t *testing.T, ctx context.Context, inspectionURL, usageURL string) {
	t.Helper()
	inspection := connect(t, ctx, inspectionURL)
	defer inspection.Close(ctx)
	var owner, volatility, config string
	var securityDefiner bool
	if err := inspection.QueryRow(ctx, `
		select routine.proowner::regrole::text, routine.provolatile::text, routine.prosecdef, coalesce(array_to_string(routine.proconfig, ','), '')
		from pg_proc as routine
		join pg_namespace as namespace on namespace.oid = routine.pronamespace
		where namespace.nspname = 'k8s_reporting'
		  and routine.proname = 'usage_sandbox_events'
		  and routine.proargtypes = '25 1184 1184'::oidvector`).Scan(&owner, &volatility, &securityDefiner, &config); err != nil {
		t.Fatalf("read usage reporting function contract: %v", err)
	}
	if owner != "k8s_reporting_owner" || volatility != "s" || !securityDefiner || config != "search_path=k8s_state, pg_catalog" {
		t.Fatalf("usage reporting function = owner:%s volatility:%s definer:%t config:%q", owner, volatility, securityDefiner, config)
	}
	assertNoPublicFunctionExecute(t, ctx, inspection, "k8s_reporting.usage_sandbox_events(text,timestamptz,timestamptz)")
	for privilege, want := range map[string]bool{"SELECT": false, "DELETE": false} {
		var allowed bool
		if err := inspection.QueryRow(ctx, `select has_table_privilege('cyclops_usage_reader', 'k8s_state.resource_event_outbox', $1)`, privilege).Scan(&allowed); err != nil || allowed != want {
			t.Fatalf("usage reader outbox %s privilege = %t err=%v, want %t", privilege, allowed, err, want)
		}
	}
	_, err := inspection.Exec(ctx, `
		insert into k8s_state.resource_event_outbox
		(event_id, cluster_id, api_group, resource, namespace, name, capsule_tenant, uid, schema_hash, event_type, watch_epoch, observed_sequence, object, observed_at)
		values
		('10000000-0000-0000-0000-000000000010', 'migration-test', 'osgym.cua.ai', 'osgymsandboxes', 'alice-ns', 'sandbox-a', 'user-alice', 'sandbox-uid-a', 'schema', 'ADDED', 2, 100, '{"metadata":{"labels":{"osgym.cua.ai/warmpool":"pool-old"}},"spec":{"vmTemplate":{"runtime":"qemu"}},"status":{"vmName":"vm-old"}}', '2026-08-17T00:00:00Z'),
		('10000000-0000-0000-0000-000000000011', 'migration-test', 'osgym.cua.ai', 'osgymsandboxes', 'alice-ns', 'sandbox-a', 'user-alice', 'sandbox-uid-a', 'schema', 'MODIFIED', 2, 101, '{"metadata":{"labels":{"osgym.cua.ai/warmpool":"pool-a"}},"spec":{"vmTemplate":{"runtime":"qemu"}},"status":{"vmName":"vm-a"}}', '2026-08-17T23:00:00Z'),
		('10000000-0000-0000-0000-000000000012', 'migration-test', 'osgym.cua.ai', 'osgymsandboxes', 'alice-ns', 'sandbox-a', 'user-alice', 'sandbox-uid-a', 'schema', 'MODIFIED', 2, 102, '{"metadata":{"labels":{},"annotations":{"osgym.cua.ai/origin-warmpool":"pool-a"}},"spec":{"vmTemplate":{"runtime":"qemu"}},"status":{"vmName":"vm-a-2"}}', '2026-08-18T06:00:00Z'),
		('10000000-0000-0000-0000-000000000013', 'migration-test', 'osgym.cua.ai', 'osgymsandboxes', 'alice-ns', 'sandbox-a', 'user-alice', 'sandbox-uid-a', 'schema', 'DELETED', 2, 103, '{"metadata":{"labels":{"osgym.cua.ai/warmpool":"pool-a"}},"spec":{"vmTemplate":{"runtime":"qemu"}},"status":{"vmName":"vm-a-2"}}', '2026-08-18T12:00:00Z'),
		('10000000-0000-0000-0000-000000000014', 'migration-test', 'osgym.cua.ai', 'osgymsandboxes', 'bob-ns', 'sandbox-b', 'user-bob', 'sandbox-uid-b', 'schema', 'ADDED', 2, 104, '{"metadata":{"labels":{"osgym.cua.ai/warmpool":"pool-b"}},"spec":{"vmTemplate":{"runtime":"qemu"}},"status":{"vmName":"vm-b"}}', '2026-08-18T08:00:00Z'),
		('10000000-0000-0000-0000-000000000015', 'migration-test', '', 'pods', 'alice-ns', 'sandbox-pod', 'user-alice', 'pod-uid', 'schema', 'ADDED', 2, 105, '{}', '2026-08-18T09:00:00Z'),
		('10000000-0000-0000-0000-000000000016', 'migration-test', 'osgym.cua.ai', 'osgymsandboxes', 'alice-ns', 'sandbox-after', 'user-alice', 'sandbox-uid-after', 'schema', 'ADDED', 2, 106, '{}', '2026-08-19T00:00:00Z'),
		('10000000-0000-0000-0000-000000000017', 'migration-test', 'osgym.cua.ai', 'osgymsandboxes', 'alice-ns', 'sandbox-b', 'user-alice', 'sandbox-uid-b', 'schema', 'ADDED', 2, 107, '{"metadata":{"labels":{"osgym.cua.ai/warmpool":"pool-b-old"}},"spec":{"vmTemplate":{"runtime":"qemu"}},"status":{"vmName":"vm-b-old"}}', '2026-08-17T20:00:00Z'),
		('10000000-0000-0000-0000-000000000018', 'migration-test', 'osgym.cua.ai', 'osgymsandboxes', 'alice-ns', 'sandbox-b', 'user-alice', 'sandbox-uid-b', 'schema', 'MODIFIED', 2, 108, '{"metadata":{"labels":{"osgym.cua.ai/warmpool":"pool-b"}},"spec":{"vmTemplate":{"runtime":"qemu"}},"status":{"vmName":"vm-b"}}', '2026-08-17T22:00:00Z'),
		('10000000-0000-0000-0000-000000000019', 'migration-test', 'osgym.cua.ai', 'osgymsandboxes', 'alice-other', 'sandbox-a', 'user-alice', 'sandbox-uid-a', 'schema', 'ADDED', 2, 109, '{"metadata":{"labels":{"osgym.cua.ai/warmpool":"pool-other-old"}},"spec":{"vmTemplate":{"runtime":"qemu"}},"status":{"vmName":"vm-other-old"}}', '2026-08-17T19:00:00Z'),
		('10000000-0000-0000-0000-000000000020', 'migration-test', 'osgym.cua.ai', 'osgymsandboxes', 'alice-other', 'sandbox-a', 'user-alice', 'sandbox-uid-a', 'schema', 'MODIFIED', 2, 110, '{"metadata":{"labels":{"osgym.cua.ai/warmpool":"pool-other"}},"spec":{"vmTemplate":{"runtime":"qemu"}},"status":{"vmName":"vm-other"}}', '2026-08-17T21:00:00Z'),
		('10000000-0000-0000-0000-000000000021', 'migration-test', 'osgym.cua.ai', 'osgymsandboxes', 'alice-ns', 'legacy-unlabeled', 'user-alice', 'legacy-unlabeled-uid', 'schema', 'ADDED', 1, 1, '{"metadata":{"labels":{}},"spec":{"vmTemplate":{"runtime":"qemu"}},"status":{"vmName":"legacy-vm"}}', '2026-08-18T10:00:00Z')`)
	if err != nil {
		t.Fatal("seed usage sandbox events")
	}

	type usageEvent struct {
		eventID, namespace, name, uid, pool, runtime, vmName, eventType string
		observedAt                                                      time.Time
	}
	connection := connect(t, ctx, usageURL)
	defer connection.Close(ctx)
	for setting, want := range map[string]string{
		"default_transaction_read_only":       "on",
		"statement_timeout":                   "10s",
		"idle_in_transaction_session_timeout": "10s",
	} {
		var got string
		if err := connection.QueryRow(ctx, "show "+setting).Scan(&got); err != nil || got != want {
			t.Fatalf("usage reader %s = %q err=%v, want %q", setting, got, err, want)
		}
	}
	rows, err := connection.Query(ctx, `select event_id, namespace, sandbox_name, sandbox_uid, pool_name, runtime, vm_name, event_type, observed_at from k8s_reporting.usage_sandbox_events($1, $2, $3)`, "user-alice", time.Date(2026, 8, 18, 0, 0, 0, 0, time.UTC), time.Date(2026, 8, 19, 0, 0, 0, 0, time.UTC))
	if err != nil {
		t.Fatalf("query usage sandbox events: %v", err)
	}
	defer rows.Close()
	var got []usageEvent
	for rows.Next() {
		var event usageEvent
		if err := rows.Scan(&event.eventID, &event.namespace, &event.name, &event.uid, &event.pool, &event.runtime, &event.vmName, &event.eventType, &event.observedAt); err != nil {
			t.Fatal("scan usage sandbox event")
		}
		event.observedAt = event.observedAt.UTC()
		got = append(got, event)
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("read usage sandbox events: %v", err)
	}
	want := []usageEvent{
		{"10000000-0000-0000-0000-000000000011", "alice-ns", "sandbox-a", "sandbox-uid-a", "pool-a", "qemu", "vm-a", "MODIFIED", time.Date(2026, 8, 17, 23, 0, 0, 0, time.UTC)},
		{"10000000-0000-0000-0000-000000000012", "alice-ns", "sandbox-a", "sandbox-uid-a", "pool-a", "qemu", "vm-a-2", "MODIFIED", time.Date(2026, 8, 18, 6, 0, 0, 0, time.UTC)},
		{"10000000-0000-0000-0000-000000000013", "alice-ns", "sandbox-a", "sandbox-uid-a", "pool-a", "qemu", "vm-a-2", "DELETED", time.Date(2026, 8, 18, 12, 0, 0, 0, time.UTC)},
		{"10000000-0000-0000-0000-000000000018", "alice-ns", "sandbox-b", "sandbox-uid-b", "pool-b", "qemu", "vm-b", "MODIFIED", time.Date(2026, 8, 17, 22, 0, 0, 0, time.UTC)},
		{"10000000-0000-0000-0000-000000000020", "alice-other", "sandbox-a", "sandbox-uid-a", "pool-other", "qemu", "vm-other", "MODIFIED", time.Date(2026, 8, 17, 21, 0, 0, 0, time.UTC)},
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("usage sandbox events = %#v, want %#v", got, want)
	}

	for _, statement := range []string{
		`select * from k8s_reporting.usage_sandbox_events('', '2026-08-18T00:00:00Z', '2026-08-19T00:00:00Z')`,
		`select * from k8s_reporting.usage_sandbox_events('user-alice', '2026-08-19T00:00:00Z', '2026-08-18T00:00:00Z')`,
		`select * from k8s_reporting.usage_sandbox_events('user-alice', '2026-07-18T00:00:00Z', '2026-08-19T00:00:00Z')`,
		`select * from k8s_state.resource_event_outbox`,
	} {
		assertStatementFails(t, ctx, usageURL, statement)
	}
	if _, err := connection.Exec(ctx, `set default_transaction_read_only = off`); err != nil {
		t.Fatal("disable usage read-only default for ACL test")
	}
	if _, err := connection.Exec(ctx, `delete from k8s_state.resource_event_outbox`); err == nil || !strings.Contains(strings.ToLower(err.Error()), "permission denied") {
		t.Fatalf("usage outbox delete error = %v, want permission denied", err)
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

func assertFilteredReservationUsageTenantContract(t *testing.T, ctx context.Context, migrationURL, metabaseURL string) {
	t.Helper()
	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, `
		set role billing_meter_owner;
		insert into billing_meter.reservation_hour_collection
			(collection_run_id, logical_key, revision, cluster_id, hour_start, hour_end, covered_seconds, discovered_sandboxes, inserted_facts, unchanged_facts, source_sha256)
		values
			('00000000-0000-0000-0000-000000000101', 'filtered-tenant-contract', 1, 'filtered-tenant-contract', '2026-08-27 00:00:00+00', '2026-08-27 01:00:00+00', 3600, 5, 5, 0, repeat('a', 64));
		insert into billing_meter.reservation_hour_fact
			(fact_id, logical_key, revision, cluster_id, capsule_tenant, namespace, sandbox_uid, sandbox_name, pool_name, runtime, hour_start, hour_end, virtual_cpu_core_seconds, virtual_memory_byte_seconds, ready_seconds, covered_seconds, scrape_interval_seconds, source_sha256, collection_run_id)
		values
			('00000000-0000-0000-0000-000000000102', 'filtered-tenant-contract-1', 1, 'filtered-tenant-contract', 'user-f039fe89-9b5f-43dc-8ccd-d100ae732246', 'contract', 'excluded-1', 'excluded-1', 'pool', 'runtime', '2026-08-27 00:00:00+00', '2026-08-27 01:00:00+00', 10, 100, 10, 3600, 60, repeat('b', 64), '00000000-0000-0000-0000-000000000101'),
			('00000000-0000-0000-0000-000000000103', 'filtered-tenant-contract-2', 1, 'filtered-tenant-contract', 'user-30a53246-881d-4f1a-8005-979f2a07933e', 'contract', 'excluded-2', 'excluded-2', 'pool', 'runtime', '2026-08-27 00:00:00+00', '2026-08-27 01:00:00+00', 20, 200, 20, 3600, 60, repeat('c', 64), '00000000-0000-0000-0000-000000000101'),
			('00000000-0000-0000-0000-000000000104', 'filtered-tenant-contract-3', 1, 'filtered-tenant-contract', 'user-0ea07f31-b7bd-4e99-b29a-2376f6fde1be', 'contract', 'excluded-3', 'excluded-3', 'pool', 'runtime', '2026-08-27 00:00:00+00', '2026-08-27 01:00:00+00', 30, 300, 30, 3600, 60, repeat('d', 64), '00000000-0000-0000-0000-000000000101'),
			('00000000-0000-0000-0000-000000000105', 'filtered-tenant-contract-4', 1, 'filtered-tenant-contract', 'user-a89b2628-9656-4ef0-bf01-e925b120ed1d', 'contract', 'excluded-4', 'excluded-4', 'pool', 'runtime', '2026-08-27 00:00:00+00', '2026-08-27 01:00:00+00', 40, 400, 40, 3600, 60, repeat('e', 64), '00000000-0000-0000-0000-000000000101'),
			('00000000-0000-0000-0000-000000000106', 'filtered-tenant-contract-5', 1, 'filtered-tenant-contract', 'included-tenant', 'contract', 'included', 'included', 'pool', 'runtime', '2026-08-27 00:00:00+00', '2026-08-27 01:00:00+00', 50, 500, 50, 3600, 60, repeat('f', 64), '00000000-0000-0000-0000-000000000101');
		reset role`); err != nil {
		t.Fatal("seed filtered reservation usage contract: ", err)
	}

	metabase := connect(t, ctx, metabaseURL)
	defer metabase.Close(ctx)
	var discoveredSandboxes, reservationFacts int
	var includedCPU, includedMemory, includedReady bool
	if err := metabase.QueryRow(ctx, `
		select
			discovered_sandboxes,
			reservation_fact_count,
			virtual_cpu_core_seconds = 50,
			virtual_memory_byte_seconds = 500,
			ready_seconds = 50
		from k8s_reporting.hourly_reservation_usage_excluding_tenants
		where cluster_id = 'filtered-tenant-contract'
		  and hour_start = '2026-08-27 00:00:00+00'`).Scan(&discoveredSandboxes, &reservationFacts, &includedCPU, &includedMemory, &includedReady); err != nil {
		t.Fatal("query filtered reservation usage view: ", err)
	}
	if discoveredSandboxes != 1 || reservationFacts != 1 || !includedCPU || !includedMemory || !includedReady {
		t.Fatalf("filtered reservation usage = sandboxes:%d facts:%d cpu:%t memory:%t ready:%t, want only included tenant", discoveredSandboxes, reservationFacts, includedCPU, includedMemory, includedReady)
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

func assertMetabaseBoundary(t *testing.T, ctx context.Context, inspectionURL, metabaseURL string) {
	t.Helper()
	inspection := connect(t, ctx, inspectionURL)
	defer inspection.Close(ctx)
	var tenantForRoleOID uint32
	if err := inspection.QueryRow(ctx, `
		select routine.oid
		from pg_proc as routine
		join pg_namespace as namespace on namespace.oid = routine.pronamespace
		where namespace.nspname = 'k8s_state'
		  and routine.proname = 'tenant_for_role'
		  and routine.proargtypes = '19'::oidvector`).Scan(&tenantForRoleOID); err != nil {
		t.Fatalf("resolve k8s_state.tenant_for_role OID: %v", err)
	}
	connection := connect(t, ctx, metabaseURL)
	defer connection.Close(ctx)
	var count int
	if err := connection.QueryRow(ctx, `select count(*) from k8s_reporting.current_resources where cluster_id = 'migration-test'`).Scan(&count); err != nil || count != 4 {
		t.Errorf("metabase cannot read reporting view: count=%d err=%v", count, err)
	}
	if err := connection.QueryRow(ctx, `select count(*) from k8s_reporting.hourly_reservation_usage`).Scan(&count); err != nil {
		t.Errorf("metabase cannot read hourly reservation usage view: err=%v", err)
	}
	if err := connection.QueryRow(ctx, `select count(*) from k8s_reporting.hourly_reservation_usage_excluding_tenants`).Scan(&count); err != nil {
		t.Errorf("metabase cannot read filtered hourly reservation usage view: err=%v", err)
	}
	var readOnly string
	if err := connection.QueryRow(ctx, `show default_transaction_read_only`).Scan(&readOnly); err != nil || readOnly != "on" {
		t.Errorf("metabase default_transaction_read_only = %q err=%v, want on", readOnly, err)
	}
	if err := connection.QueryRow(ctx, `select count(*) from pg_auth_members where member = 'k8s_metabase'::regrole`).Scan(&count); err != nil || count != 0 {
		t.Errorf("k8s_metabase membership count = %d err=%v, want 0", count, err)
	}
	var canExecute bool
	if err := connection.QueryRow(ctx, `select has_function_privilege('k8s_metabase'::regrole, $1::oid, 'EXECUTE')`, tenantForRoleOID).Scan(&canExecute); err != nil || canExecute {
		t.Errorf("k8s_metabase execute k8s_state.tenant_for_role = %t err=%v, want false", canExecute, err)
	}
	if _, err := connection.Exec(ctx, `set default_transaction_read_only = off`); err != nil {
		t.Fatalf("disable metabase read-only default: %v", err)
	}
	if _, err := connection.Exec(ctx, `delete from k8s_reporting.current_resources`); err == nil {
		t.Error("metabase can write reporting view after disabling read-only default")
	}
	for _, statement := range []string{
		`select 1 from k8s_state.resource_state limit 1`,
		`select 1 from k8s_api.current_resources limit 1`,
		`select 1 from public.github_trust_policies limit 1`,
		`delete from k8s_reporting.current_resources`,
		`delete from k8s_reporting.hourly_reservation_usage`,
		`delete from k8s_reporting.hourly_reservation_usage_excluding_tenants`,
	} {
		assertStatementFails(t, ctx, metabaseURL, statement)
	}
}

func assertInitialMigrationRejectsPublicSecurityDefiner(t *testing.T, ctx context.Context, migrationURL string, credentials CredentialURLs) {
	t.Helper()
	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	if _, err := connection.Exec(ctx, `create function public.migration_public_definer() returns integer language sql security definer as $$ select 1 $$`); err != nil {
		t.Fatal("create public security-definer fixture")
	}
	if err := Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}); err == nil || !strings.Contains(err.Error(), "PUBLIC-executable SECURITY DEFINER routine") {
		t.Fatalf("Run() error = %v, want PUBLIC-executable SECURITY DEFINER routine", err)
	}
	var reportingSchemaExists, metabaseRoleExists bool
	if err := connection.QueryRow(ctx, `select exists (select 1 from pg_namespace where nspname = 'k8s_reporting'), exists (select 1 from pg_roles where rolname = 'k8s_metabase')`).Scan(&reportingSchemaExists, &metabaseRoleExists); err != nil {
		t.Fatal("check rolled-back reporting migration")
	}
	if reportingSchemaExists || metabaseRoleExists {
		t.Fatalf("failed migration left reporting authority behind: schema=%t role=%t", reportingSchemaExists, metabaseRoleExists)
	}
	if _, err := connection.Exec(ctx, `revoke execute on function public.migration_public_definer() from public`); err != nil {
		t.Fatal("revoke public execute from security-definer fixture")
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

func reportingOwnerACLRows(t *testing.T, ctx context.Context, connection *pgx.Conn) string {
	t.Helper()
	var rows string
	if err := connection.QueryRow(ctx, `
		select coalesce(string_agg(object_identity || ':' || privilege_type, ',' order by object_identity, privilege_type), '')
		from (
			select 'schema:' || namespace.nspname as object_identity, acl.privilege_type
			from pg_namespace as namespace
			join lateral aclexplode(namespace.nspacl) as acl on true
			where namespace.nspname = 'k8s_reporting' and acl.grantee = namespace.nspowner
			union all
			select 'relation:' || namespace.nspname || '.' || relation.relname, acl.privilege_type
			from pg_class as relation
			join pg_namespace as namespace on namespace.oid = relation.relnamespace
			join lateral aclexplode(relation.relacl) as acl on true
			where namespace.nspname = 'k8s_reporting' and relation.relname = 'current_resources' and acl.grantee = relation.relowner
		) as owner_acl`).Scan(&rows); err != nil {
		t.Fatalf("read reporting owner ACL rows: %v", err)
	}
	return rows
}

func assertExactReportingACLContract(t *testing.T, ctx context.Context, connection *pgx.Conn) {
	t.Helper()
	var differences int
	if err := connection.QueryRow(ctx, `
		with actual as (
			select 'schema:' || namespace.nspname as object_identity, acl.privilege_type, acl.grantee::regrole::text, acl.grantor::regrole::text, acl.is_grantable
			from pg_namespace as namespace
			join lateral aclexplode(namespace.nspacl) as acl on true
			where acl.grantee <> namespace.nspowner
			  and acl.grantee in ('k8s_reporting_owner'::regrole, 'k8s_metabase'::regrole, 'cyclops_usage_reader'::regrole)
			union all
			select 'relation:' || namespace.nspname || '.' || relation.relname, acl.privilege_type, acl.grantee::regrole::text, acl.grantor::regrole::text, acl.is_grantable
			from pg_class as relation
			join pg_namespace as namespace on namespace.oid = relation.relnamespace
			join lateral aclexplode(relation.relacl) as acl on true
			where relation.relkind in ('r', 'p', 'v', 'm', 'f', 'S')
			  and acl.grantee <> relation.relowner
			  and acl.grantee in ('k8s_reporting_owner'::regrole, 'k8s_metabase'::regrole, 'cyclops_usage_reader'::regrole)
			union all
			select 'routine:' || routine.oid::regprocedure::text, acl.privilege_type, acl.grantee::regrole::text, acl.grantor::regrole::text, acl.is_grantable
			from pg_proc as routine
			join pg_namespace as namespace on namespace.oid = routine.pronamespace
			join lateral aclexplode(routine.proacl) as acl on true
			where acl.grantee <> routine.proowner
			  and acl.grantee in ('k8s_reporting_owner'::regrole, 'k8s_metabase'::regrole, 'cyclops_usage_reader'::regrole)
		), expected as (
			values
				('schema:k8s_state', 'USAGE', 'k8s_reporting_owner', 'k8s_state_owner', false),
				('relation:k8s_state.resource_state', 'SELECT', 'k8s_reporting_owner', 'k8s_state_owner', false),
				('relation:k8s_state.resource_event_outbox', 'SELECT', 'k8s_reporting_owner', 'k8s_state_owner', false),
				('schema:billing_meter', 'USAGE', 'k8s_reporting_owner', 'billing_meter_owner', false),
				('relation:billing_meter.reservation_hour_current', 'SELECT', 'k8s_reporting_owner', 'billing_meter_owner', false),
				('relation:billing_meter.reservation_hour_collection_current', 'SELECT', 'k8s_reporting_owner', 'billing_meter_owner', false),
				('schema:billing_meter', 'USAGE', 'k8s_metabase', 'billing_meter_owner', false),
				('relation:billing_meter.reservation_hour_collection', 'SELECT', 'k8s_metabase', 'billing_meter_owner', false),
				('relation:billing_meter.reservation_hour_fact', 'SELECT', 'k8s_metabase', 'billing_meter_owner', false),
				('relation:billing_meter.reservation_hour_current', 'SELECT', 'k8s_metabase', 'billing_meter_owner', false),
				('relation:billing_meter.reservation_hour_collection_current', 'SELECT', 'k8s_metabase', 'billing_meter_owner', false),
				('schema:k8s_reporting', 'USAGE', 'k8s_metabase', 'k8s_reporting_owner', false),
				('relation:k8s_reporting.current_resources', 'SELECT', 'k8s_metabase', 'k8s_reporting_owner', false),
				('relation:k8s_reporting.hourly_reservation_usage', 'SELECT', 'k8s_metabase', 'k8s_reporting_owner', false),
				('relation:k8s_reporting.hourly_reservation_usage_excluding_tenants', 'SELECT', 'k8s_metabase', 'k8s_reporting_owner', false),
				('schema:k8s_reporting', 'USAGE', 'cyclops_usage_reader', 'k8s_reporting_owner', false),
				('routine:k8s_reporting.usage_sandbox_events(text,timestamp with time zone,timestamp with time zone)', 'EXECUTE', 'cyclops_usage_reader', 'k8s_reporting_owner', false),
				('routine:k8s_reporting.reservation_hour_facts(text,timestamp with time zone,timestamp with time zone)', 'EXECUTE', 'cyclops_usage_reader', 'k8s_reporting_owner', false),
				('routine:k8s_reporting.reservation_meter_status(text,timestamp with time zone,timestamp with time zone)', 'EXECUTE', 'cyclops_usage_reader', 'k8s_reporting_owner', false)
		)
		select count(*) from (
			(select * from actual except select * from expected)
			union all
			(select * from expected except select * from actual)
		) as difference`).Scan(&differences); err != nil {
		t.Fatalf("read reporting ACL contract: %v", err)
	}
	if differences != 0 {
		t.Fatalf("reporting ACL contract differences = %d, want 0", differences)
	}

	var reportingObjectDrift bool
	if err := connection.QueryRow(ctx, `
		select exists (
			select 1
			from pg_namespace as namespace
			join lateral aclexplode(namespace.nspacl) as acl on true
			where namespace.nspname = 'k8s_reporting'
			  and acl.grantee <> namespace.nspowner
			  and (acl.grantee not in ('k8s_metabase'::regrole, 'cyclops_usage_reader'::regrole) or acl.privilege_type <> 'USAGE' or acl.is_grantable)
			union all
			select 1
			from pg_class as relation
			join pg_namespace as namespace on namespace.oid = relation.relnamespace
			join lateral aclexplode(relation.relacl) as acl on true
			where namespace.nspname = 'k8s_reporting' and relation.relname in ('current_resources', 'hourly_reservation_usage', 'hourly_reservation_usage_excluding_tenants')
			  and acl.grantee <> relation.relowner
			  and (acl.grantee <> 'k8s_metabase'::regrole or acl.privilege_type <> 'SELECT' or acl.is_grantable)
			union all
			select 1
			from pg_proc as routine
			join pg_namespace as namespace on namespace.oid = routine.pronamespace
			join lateral aclexplode(routine.proacl) as acl on true
			where namespace.nspname = 'k8s_reporting'
			  and routine.proname in ('usage_sandbox_events', 'reservation_hour_facts', 'reservation_meter_status')
			  and routine.proargtypes = '25 1184 1184'::oidvector
			  and acl.grantee <> routine.proowner
			  and (acl.grantee <> 'cyclops_usage_reader'::regrole or acl.privilege_type <> 'EXECUTE' or acl.is_grantable)
		)`).Scan(&reportingObjectDrift); err != nil {
		t.Fatalf("read reporting schema and view ACLs: %v", err)
	}
	if reportingObjectDrift {
		t.Fatal("reporting schema or view retains a PUBLIC or unexpected grantee ACL")
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
