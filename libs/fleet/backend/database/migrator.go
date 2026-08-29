package database

import (
	"context"
	"crypto/sha256"
	"embed"
	"encoding/hex"
	"errors"
	"fmt"
	"io/fs"
	"log/slog"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

//go:embed all:migrations
var migrationFS embed.FS

var (
	ErrInvalidConfiguration = errors.New("invalid database configuration")
	ErrUnavailable          = errors.New("database unavailable")
)

type ErrorClassification struct {
	Class     string
	Retryable bool
}

func ClassifyError(err error) ErrorClassification {
	switch {
	case errors.Is(err, ErrInvalidConfiguration):
		return ErrorClassification{Class: "invalid_configuration"}
	case errors.Is(err, ErrUnavailable):
		return ErrorClassification{Class: "unavailable", Retryable: true}
	default:
		return ErrorClassification{Class: "internal"}
	}
}

type Config struct {
	MigrationURL string
	Credentials  CredentialURLs
}

type CredentialURLs struct {
	Application string
	Writer      string
	Exporter    string
	RoleAdmin   string
	Metabase    string
	Usage       string
	Meter       string
}

type migrationFile struct {
	Version int64
	Name    string
	SQL     string
	SHA256  string
}

type appliedMigration struct {
	Version int64
	Name    string
	SHA256  string
}

type credential struct {
	Role     string
	Password string
}

type migrationEvent struct {
	Version    int64
	Filename   string
	SHA256     string
	DurationMS int64
}

type credentialEvent struct {
	Role       string
	DurationMS int64
}

type staticRoleContract struct {
	role            string
	login           bool
	inherit         bool
	createRole      bool
	createDB        bool
	connectionLimit int
	validUntil      staticRoleValidUntil
}

type staticRoleAttributes struct {
	login           bool
	inherit         bool
	createRole      bool
	createDB        bool
	connectionLimit int
	validUntil      string
	super           bool
	replication     bool
	bypassRLS       bool
}

type staticMembershipContract struct {
	role, member        string
	admin, inherit, set bool
}

type staticMembershipGrant struct {
	grantor             string
	grantorSuperuser    bool
	admin, inherit, set bool
}

type dynamicTenantRole struct {
	name            string
	registered      bool
	login           bool
	inherit         bool
	createRole      bool
	createDB        bool
	super           bool
	replication     bool
	bypassRLS       bool
	connectionLimit int
	validUntil      string
}

type roleReconciliationEvent struct {
	Role       string
	DurationMS int64
}

type membershipReconciliationEvent struct {
	Role       string
	DurationMS int64
}
type migrationSummary struct {
	DatabaseHost string
	DatabaseName string
	Current      int64
	Target       int64
	Pending      int
	Applied      int
	Skipped      int
	Started      time.Time
	Result       string
}

var expectedCredentialRoles = map[string]string{
	"application": "cyclops_app",
	"writer":      "k8s_state_writer",
	"exporter":    "k8s_state_exporter",
	"role-admin":  "k8s_role_admin",
	"metabase":    "k8s_metabase",
	"usage":       "cyclops_usage_reader",
	"meter":       "cyclops_meter_writer",
}

const selectAppliedMigrationsStatement = `select version, filename, sha256 from cyclops_migrations.applied_migrations order by application_order`

const insertAppliedMigrationStatement = `insert into cyclops_migrations.applied_migrations (version, filename, sha256) values ($1, $2, $3)`

func embeddedMigrations() ([]migrationFile, error) {
	entries, err := fs.ReadDir(migrationFS, "migrations")
	if err != nil {
		return nil, fmt.Errorf("list database migrations: %w", err)
	}

	files := make([]migrationFile, 0, len(entries))
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".sql") {
			continue
		}

		prefix, _, ok := strings.Cut(entry.Name(), "_")
		if !ok || len(prefix) != 6 {
			return nil, fmt.Errorf("migration %q must start with a six-digit version", entry.Name())
		}
		version, err := strconv.ParseInt(prefix, 10, 64)
		if err != nil || version < 1 {
			return nil, errors.Join(fmt.Errorf("migration %q has invalid version", entry.Name()), err)

		}

		contents, err := migrationFS.ReadFile("migrations/" + entry.Name())
		if err != nil {
			return nil, fmt.Errorf("read migration %s: %w", entry.Name(), err)
		}
		digest := sha256.Sum256(contents)
		files = append(files, migrationFile{
			Version: version,
			Name:    entry.Name(),
			SQL:     string(contents),
			SHA256:  hex.EncodeToString(digest[:]),
		})
	}

	sort.Slice(files, func(i, j int) bool { return files[i].Version < files[j].Version })
	for index, file := range files {
		expected := int64(index + 1)
		if file.Version != expected {
			return nil, fmt.Errorf("migration sequence gap: expected %06d, got %06d", expected, file.Version)
		}
	}
	return files, nil
}

func checkAppliedMigration(file migrationFile, applied appliedMigration) error {
	if applied.Name != file.Name {
		return fmt.Errorf("migration %06d filename changed after application: recorded %s, current %s", file.Version, applied.Name, file.Name)
	}
	if applied.SHA256 != file.SHA256 {
		return fmt.Errorf("migration %s changed after application: recorded %s, current %s", file.Name, applied.SHA256, file.SHA256)
	}
	return nil
}

func validateAppliedMigrations(files []migrationFile, applied []appliedMigration) error {
	filesByVersion := make(map[int64]migrationFile, len(files))
	for _, file := range files {
		filesByVersion[file.Version] = file
	}
	for _, row := range applied {
		file, ok := filesByVersion[row.Version]
		if !ok {
			return fmt.Errorf("migration ledger contains applied version %06d with no embedded migration", row.Version)
		}
		if err := checkAppliedMigration(file, row); err != nil {
			return err
		}
	}

	for index := 1; index < len(applied); index++ {
		previous := applied[index-1].Version
		current := applied[index].Version
		if current == previous {
			return fmt.Errorf("migration ledger contains duplicate version %06d", current)
		}
		if current < previous {
			return fmt.Errorf("migration ledger rows are out of order: version %06d follows %06d", current, previous)
		}
	}

	for index, row := range applied {
		expected := int64(index + 1)
		if row.Version != expected {
			return fmt.Errorf("migration ledger version gap: expected %06d, got %06d", expected, row.Version)
		}
	}
	return nil
}

func migrationTargetVersion(files []migrationFile) int64 {
	if len(files) == 0 {
		return 0
	}
	return files[len(files)-1].Version
}

func migrationCurrentVersion(applied []appliedMigration) int64 {
	var current int64
	for _, row := range applied {
		if row.Version > current {
			current = row.Version
		}
	}
	return current
}

func databaseTarget(connectionConfig *pgx.ConnConfig) string {
	if connectionConfig.Host == "" && connectionConfig.Database == "" {
		return ""
	}
	return fmt.Sprintf(" (database_host=%s database_name=%s)", connectionConfig.Host, connectionConfig.Database)
}

func parseMigrationConfig(url string) (*pgx.ConnConfig, error) {
	connectionConfig, err := pgx.ParseConfig(url)
	if err != nil {
		return nil, fmt.Errorf("%w: parse migration database URL: %w", ErrInvalidConfiguration, err)

	}
	return connectionConfig, nil
}

func logMigrationSummary(summary migrationSummary) {
	slog.Info("database migration summary",
		"database_host", summary.DatabaseHost,
		"database_name", summary.DatabaseName,
		"current_version", summary.Current,
		"target_version", summary.Target,
		"pending", summary.Pending,
		"applied", summary.Applied,
		"skipped", summary.Skipped,
		"duration_ms", time.Since(summary.Started).Milliseconds(),
		"result", summary.Result,
	)
}

func Run(ctx context.Context, config Config) (runErr error) {
	credentials, err := parseCredentialURLs(config.Credentials)
	if err != nil {
		return err
	}
	files, err := embeddedMigrations()
	if err != nil {
		return err
	}

	connectionConfig, err := parseMigrationConfig(config.MigrationURL)
	if err != nil {
		return err
	}
	summary := migrationSummary{
		DatabaseHost: connectionConfig.Host,
		DatabaseName: connectionConfig.Database,
		Target:       migrationTargetVersion(files),
		Pending:      len(files),
		Started:      time.Now(),
		Result:       "failed",
	}
	defer func() { logMigrationSummary(summary) }()

	connection, err := pgx.ConnectConfig(ctx, connectionConfig)
	if err != nil {
		return fmt.Errorf("%w%s: %w", ErrUnavailable, databaseTarget(connectionConfig), err)

	}
	defer connection.Close(ctx)

	transaction, err := connection.Begin(ctx)
	if err != nil {
		return fmt.Errorf("begin database migrations: %w", err)
	}
	defer transaction.Rollback(ctx)

	ddl := newRuntimeDDL(transaction)
	started := time.Now()
	if err := ddl.ensureMigrationLedger(ctx); err != nil {
		return fmt.Errorf("prepare migration ledger: %w", err)
	}
	slog.Info("database migration advisory lock acquired",
		"database_host", connectionConfig.Host,
		"database_name", connectionConfig.Database,
		"duration_ms", time.Since(started).Milliseconds(),
	)

	rows, err := transaction.Query(ctx, selectAppliedMigrationsStatement)
	if err != nil {
		return fmt.Errorf("read migration ledger: %w", err)
	}
	applied := make([]appliedMigration, 0)
	for rows.Next() {
		var row appliedMigration
		if err := rows.Scan(&row.Version, &row.Name, &row.SHA256); err != nil {
			rows.Close()
			return fmt.Errorf("read migration ledger: %w", err)
		}
		applied = append(applied, row)
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		return fmt.Errorf("read migration ledger: %w", err)
	}
	rows.Close()
	if err := validateAppliedMigrations(files, applied); err != nil {
		return err
	}

	appliedByVersion := make(map[int64]appliedMigration, len(applied))
	for _, row := range applied {
		appliedByVersion[row.Version] = row
	}
	summary.Current = migrationCurrentVersion(applied)
	summary.Pending = len(files) - len(applied)

	migrationEvents := make([]migrationEvent, 0, summary.Pending)
	for _, file := range files {
		started := time.Now()
		if _, ok := appliedByVersion[file.Version]; ok {
			summary.Skipped++
			slog.Info("database migration",
				"version", file.Version,
				"filename", file.Name,
				"sha256", file.SHA256,
				"duration_ms", time.Since(started).Milliseconds(),
				"result", "skipped",
			)
			continue
		}

		file, err = prepareMigrationExecution(file)
		if err != nil {
			return fmt.Errorf("prepare migration %s: %w", file.Name, err)
		}
		if _, err := transaction.Exec(ctx, file.SQL); err != nil {
			return fmt.Errorf("apply migration %s: %w", file.Name, err)
		}
		if _, err := transaction.Exec(ctx,
			insertAppliedMigrationStatement,
			file.Version, file.Name, file.SHA256,
		); err != nil {
			return fmt.Errorf("record migration %s: %w", file.Name, err)
		}
		migrationEvents = append(migrationEvents, migrationEvent{
			Version:    file.Version,
			Filename:   file.Name,
			SHA256:     file.SHA256,
			DurationMS: time.Since(started).Milliseconds(),
		})
	}

	roleEvents, membershipEvents, err := reconcileStaticRoleContracts(ctx, transaction)
	if err != nil {
		return fmt.Errorf("reconcile static role contracts: %w", err)
	}
	if err := reconcileReportingBoundary(ctx, transaction); err != nil {
		return fmt.Errorf("reconcile reporting boundary: %w", err)
	}
	if err := validateNoPublicSecurityDefiner(ctx, transaction); err != nil {
		return fmt.Errorf("validate security definer boundary: %w", err)
	}
	credentialEvents, err := reconcilePasswords(ctx, transaction, credentials)
	if err != nil {
		return fmt.Errorf("reconcile database credentials: %w", err)
	}
	if err := transaction.Commit(ctx); err != nil {
		return fmt.Errorf("commit database migrations: %w", err)
	}

	for _, event := range migrationEvents {
		slog.Info("database migration",
			"version", event.Version,
			"filename", event.Filename,
			"sha256", event.SHA256,
			"duration_ms", event.DurationMS,
			"result", "applied",
		)
	}
	for _, event := range roleEvents {
		slog.Info("database role reconciled",
			"role", event.Role,
			"duration_ms", event.DurationMS,
			"result", "reconciled",
		)
	}
	for _, event := range membershipEvents {
		slog.Info("database role membership reconciled",
			"role", event.Role,
			"duration_ms", event.DurationMS,
			"result", "reconciled",
		)
	}
	for _, event := range credentialEvents {
		slog.Info("database credential reconciled",
			"role", event.Role,
			"duration_ms", event.DurationMS,
			"result", "reconciled",
		)
	}
	summary.Applied = len(migrationEvents)
	summary.Result = "success"
	return nil
}

func parseCredentialURLs(urls CredentialURLs) ([]credential, error) {
	inputs := []struct {
		Name string
		URL  string
	}{
		{Name: "application", URL: urls.Application},
		{Name: "writer", URL: urls.Writer},
		{Name: "exporter", URL: urls.Exporter},
		{Name: "role-admin", URL: urls.RoleAdmin},
		{Name: "metabase", URL: urls.Metabase},
		{Name: "usage", URL: urls.Usage},
		{Name: "meter", URL: urls.Meter},
	}

	credentials := make([]credential, 0, len(inputs))
	for _, input := range inputs {
		connectionConfig, err := pgx.ParseConfig(input.URL)
		if err != nil {
			return nil, fmt.Errorf("%w: parse %s credential database URL: %w", ErrInvalidConfiguration, input.Name, err)

		}
		expectedRole := expectedCredentialRoles[input.Name]
		if connectionConfig.User != expectedRole {
			return nil, fmt.Errorf("%s credential database URL must use the expected role", input.Name)
		}
		if connectionConfig.Password == "" {
			return nil, fmt.Errorf("%s credential database URL must include a password", input.Name)
		}
		credentials = append(credentials, credential{Role: expectedRole, Password: connectionConfig.Password})
	}
	return credentials, nil
}

func staticRoleContracts() []staticRoleContract {
	return []staticRoleContract{
		{role: "cyclops_app", login: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity},
		{role: "k8s_state_owner", inherit: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity},
		{role: "k8s_state_writer", login: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity},
		{role: "k8s_state_exporter", login: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity},
		{role: "k8s_query_tenant", inherit: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity},
		{role: "k8s_query_admin", inherit: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity},
		{role: "k8s_role_admin", login: true, createRole: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity},
		{role: "k8s_reporting_owner", inherit: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity},
		{role: "billing_meter_owner", inherit: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity},
		{role: "k8s_metabase", login: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity},
		{role: "cyclops_usage_reader", login: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity},
		{role: "cyclops_meter_writer", login: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity},
	}
}

func staticRoleSettingsContracts() map[string]staticRoleSettingsContract {
	return map[string]staticRoleSettingsContract{
		"cyclops_usage_reader": {
			staticRoleSettingDefaultTransactionReadOnly:      "on",
			staticRoleSettingStatementTimeout:                "10000ms",
			staticRoleSettingIdleInTransactionSessionTimeout: "10000ms",
		},
		"k8s_metabase": {
			staticRoleSettingDefaultTransactionReadOnly:      "on",
			staticRoleSettingStatementTimeout:                "20000ms",
			staticRoleSettingIdleInTransactionSessionTimeout: "20000ms",
		},
	}
}

func staticMembershipContracts(migrationOwner string) []staticMembershipContract {
	return []staticMembershipContract{
		{role: "k8s_state_owner", member: migrationOwner, admin: false, inherit: false, set: true},
		{role: "k8s_reporting_owner", member: migrationOwner, admin: false, inherit: false, set: true},
		{role: "billing_meter_owner", member: migrationOwner, admin: false, inherit: false, set: true},
		{role: "k8s_query_tenant", member: "k8s_role_admin", admin: true, inherit: false, set: false},
		{role: "k8s_query_admin", member: "k8s_reporting_owner", admin: false, inherit: true, set: false},
	}
}

func readStaticRoleAttributes(ctx context.Context, transaction pgx.Tx, role string) (staticRoleAttributes, error) {
	var attributes staticRoleAttributes
	err := transaction.QueryRow(ctx, `
		select rolcanlogin, rolinherit, rolcreaterole, rolcreatedb, rolconnlimit, coalesce(rolvaliduntil::text, 'infinity'), rolsuper, rolreplication, rolbypassrls
		from pg_roles
		where rolname = $1`, role).Scan(
		&attributes.login,
		&attributes.inherit,
		&attributes.createRole,
		&attributes.createDB,
		&attributes.connectionLimit,
		&attributes.validUntil,
		&attributes.super,
		&attributes.replication,
		&attributes.bypassRLS,
	)
	if err != nil {
		return staticRoleAttributes{}, fmt.Errorf("read static role %s attributes: %w", role, err)
	}
	return attributes, nil
}

type reportingACL struct {
	object          reportingObject
	routineIdentity string
	owner           string
	privilege       reportingPrivilege
	grantee         string
	grantor         string
	grantable       bool
}

func reconcileReportingBoundary(ctx context.Context, transaction pgx.Tx) error {
	if err := validateReportingBoundary(ctx, transaction); err != nil {
		return err
	}

	var migrationOwner string
	if err := transaction.QueryRow(ctx, `select current_user`).Scan(&migrationOwner); err != nil {
		return fmt.Errorf("read migration role for reporting ACL reconciliation: %w", err)
	}
	acls, err := readReportingACLs(ctx, transaction)
	if err != nil {
		return err
	}
	for _, acl := range acls {
		if isExpectedReportingACL(acl) {
			continue
		}
		if !isSafeReportingACLAuthority(acl, migrationOwner) {
			return fmt.Errorf("unsafe reporting ACL on %s: privilege=%s grantee=%s grantor=%s owner=%s; migration authority cannot safely revoke this grant", reportingObjectDescription(acl.object), acl.privilege.sqlName(), acl.grantee, acl.grantor, acl.owner)
		}
	}

	for _, acl := range acls {
		if isExpectedReportingACL(acl) {
			continue
		}
		if err := newRuntimeDDL(transaction).revokeReportingACL(ctx, acl); err != nil {
			return err
		}
	}
	for _, acl := range expectedReportingACLs() {
		if acl.object.kind == reportingObjectRoutine {
			continue
		}
		if err := newRuntimeDDL(transaction).grantReportingACL(ctx, acl); err != nil {
			return fmt.Errorf("reconcile reporting ACLs: %w", err)
		}
	}
	if err := reconcileUsageRoutineExecute(ctx, transaction); err != nil {
		return err
	}

	finalACLs, err := readReportingACLs(ctx, transaction)
	if err != nil {
		return err
	}
	for _, acl := range finalACLs {
		if !isExpectedReportingACL(acl) {
			return fmt.Errorf("reporting ACL contract drift remains on %s: privilege=%s grantee=%s grantor=%s owner=%s", reportingObjectDescription(acl.object), acl.privilege.sqlName(), acl.grantee, acl.grantor, acl.owner)
		}
	}
	for _, expected := range expectedReportingACLs() {
		if !containsReportingACL(finalACLs, expected) {
			return fmt.Errorf("reporting ACL contract is missing %s on %s for %s", expected.privilege.sqlName(), reportingObjectDescription(expected.object), expected.grantee)
		}
	}
	return nil
}

func validateReportingBoundary(ctx context.Context, transaction pgx.Tx) error {
	var schemaOwner string
	if err := transaction.QueryRow(ctx, `select nspowner::regrole::text from pg_namespace where nspname = 'k8s_reporting'`).Scan(&schemaOwner); err != nil {
		return fmt.Errorf("read reporting schema: %w", err)
	}
	if schemaOwner != "k8s_reporting_owner" {
		return fmt.Errorf("reporting schema owner is %s, want k8s_reporting_owner", schemaOwner)
	}

	for _, relationName := range []string{"current_resources", "hourly_reservation_usage", "hourly_reservation_usage_excluding_tenants"} {
		var relationKind, viewOwner string
		if err := transaction.QueryRow(ctx, `
			select relation.relkind::text, relation.relowner::regrole::text
			from pg_class as relation
			join pg_namespace as namespace on namespace.oid = relation.relnamespace
			where namespace.nspname = 'k8s_reporting' and relation.relname = $1`, relationName).Scan(&relationKind, &viewOwner); err != nil {
			return fmt.Errorf("read reporting relation %s: %w", relationName, err)
		}
		if relationKind != "v" {
			return fmt.Errorf("reporting relation %s has kind %s, want view", relationName, relationKind)
		}
		if viewOwner != "k8s_reporting_owner" {
			return fmt.Errorf("reporting view owner for %s is %s, want k8s_reporting_owner", relationName, viewOwner)
		}
	}
	return nil
}

func readReportingACLs(ctx context.Context, transaction pgx.Tx) ([]reportingACL, error) {
	rows, err := transaction.Query(ctx, `
		select object_type, schema_name, object_name, routine_oid, routine_identity, owner_name, privilege_type, grantee_name, grantor_name, is_grantable
		from (
			select
				'schema'::text as object_type,
				namespace.nspname as schema_name,
				''::text as object_name,
				0::oid as routine_oid,
				''::text as routine_identity,
				namespace.nspowner::regrole::text as owner_name,
				acl.privilege_type,
				case when acl.grantee = 0 then 'PUBLIC' else acl.grantee::regrole::text end as grantee_name,
				case when acl.grantor = 0 then 'PUBLIC' else acl.grantor::regrole::text end as grantor_name,
				acl.is_grantable
			from pg_namespace as namespace
			join lateral aclexplode(namespace.nspacl) as acl on true
			where namespace.nspname !~ '^pg_'
			  and namespace.nspname <> 'information_schema'
			  and acl.grantee <> namespace.nspowner
			  and (acl.grantee in ('k8s_reporting_owner'::regrole, 'k8s_metabase'::regrole, 'cyclops_usage_reader'::regrole)
			       or (namespace.nspname = 'k8s_reporting' and acl.grantee <> namespace.nspowner))
			union all
			select
				case when relation.relkind = 'S' then 'sequence' else 'relation' end,
				namespace.nspname,
				relation.relname,
				0::oid,
				''::text,
				relation.relowner::regrole::text,
				acl.privilege_type,
				case when acl.grantee = 0 then 'PUBLIC' else acl.grantee::regrole::text end,
				case when acl.grantor = 0 then 'PUBLIC' else acl.grantor::regrole::text end,
				acl.is_grantable
			from pg_class as relation
			join pg_namespace as namespace on namespace.oid = relation.relnamespace
			join lateral aclexplode(relation.relacl) as acl on true
			where namespace.nspname !~ '^pg_'
			  and namespace.nspname <> 'information_schema'
			  and acl.grantee <> relation.relowner
			  and relation.relkind in ('r', 'p', 'v', 'm', 'f', 'S')
			  and (acl.grantee in ('k8s_reporting_owner'::regrole, 'k8s_metabase'::regrole, 'cyclops_usage_reader'::regrole)
			       or (namespace.nspname = 'k8s_reporting' and relation.relname in ('current_resources', 'hourly_reservation_usage', 'hourly_reservation_usage_excluding_tenants') and acl.grantee <> relation.relowner))
			union all
			select
				'routine'::text,
				''::text,
				''::text,
				routine.oid,
				case
					when namespace.nspname = 'k8s_reporting'
					 and routine.proname in ('usage_sandbox_events', 'reservation_hour_facts', 'reservation_meter_status')
					 and routine.proargtypes = '25 1184 1184'::oidvector then routine.proname
					else routine.oid::regprocedure::text
				end,
				routine.proowner::regrole::text,
				acl.privilege_type,
				case when acl.grantee = 0 then 'PUBLIC' else acl.grantee::regrole::text end,
				case when acl.grantor = 0 then 'PUBLIC' else acl.grantor::regrole::text end,
				acl.is_grantable
			from pg_proc as routine
			join pg_namespace as namespace on namespace.oid = routine.pronamespace
			join lateral aclexplode(routine.proacl) as acl on true
			where namespace.nspname !~ '^pg_'
			  and namespace.nspname <> 'information_schema'
			  and acl.grantee <> routine.proowner
			  and (
				acl.grantee in ('k8s_reporting_owner'::regrole, 'k8s_metabase'::regrole, 'cyclops_usage_reader'::regrole)
				or (
					namespace.nspname = 'k8s_reporting'
					and routine.proname in ('usage_sandbox_events', 'reservation_hour_facts', 'reservation_meter_status')
					and routine.proargtypes = '25 1184 1184'::oidvector
				)
			  )
		) as reporting_acl
		order by object_type, schema_name, object_name, routine_oid, routine_identity, privilege_type, grantee_name, grantor_name`)
	if err != nil {
		return nil, fmt.Errorf("enumerate reporting ACLs: %w", err)
	}
	defer rows.Close()

	acls := make([]reportingACL, 0)
	for rows.Next() {
		var acl reportingACL
		var objectKindSQL, privilegeSQL string
		if err := rows.Scan(
			&objectKindSQL,
			&acl.object.schema,
			&acl.object.name,
			&acl.object.routineOID,
			&acl.routineIdentity,
			&acl.owner,
			&privilegeSQL,
			&acl.grantee,
			&acl.grantor,
			&acl.grantable,
		); err != nil {
			return nil, fmt.Errorf("read reporting ACL: %w", err)
		}
		var ok bool
		acl.object.kind, ok = reportingObjectKindFromSQL(objectKindSQL)
		if !ok {
			return nil, fmt.Errorf("read reporting ACL with unsupported object kind %q", objectKindSQL)
		}
		acl.privilege, ok = reportingPrivilegeFromSQL(acl.object.kind, privilegeSQL)
		if !ok {
			return nil, fmt.Errorf("read reporting ACL with unsupported %s privilege %q", reportingObjectDescription(acl.object), privilegeSQL)
		}
		acls = append(acls, acl)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("read reporting ACLs: %w", err)
	}
	return acls, nil
}

func expectedReportingACLs() []reportingACL {
	return []reportingACL{
		{object: reportingObject{kind: reportingObjectSchema, schema: "k8s_state"}, owner: "k8s_state_owner", privilege: reportingPrivilegeUsage, grantee: "k8s_reporting_owner", grantor: "k8s_state_owner"},
		{object: reportingObject{kind: reportingObjectRelation, schema: "k8s_state", name: "resource_state"}, owner: "k8s_state_owner", privilege: reportingPrivilegeSelect, grantee: "k8s_reporting_owner", grantor: "k8s_state_owner"},
		{object: reportingObject{kind: reportingObjectRelation, schema: "k8s_state", name: "resource_event_outbox"}, owner: "k8s_state_owner", privilege: reportingPrivilegeSelect, grantee: "k8s_reporting_owner", grantor: "k8s_state_owner"},
		{object: reportingObject{kind: reportingObjectSchema, schema: "billing_meter"}, owner: "billing_meter_owner", privilege: reportingPrivilegeUsage, grantee: "k8s_reporting_owner", grantor: "billing_meter_owner"},
		{object: reportingObject{kind: reportingObjectRelation, schema: "billing_meter", name: "reservation_hour_current"}, owner: "billing_meter_owner", privilege: reportingPrivilegeSelect, grantee: "k8s_reporting_owner", grantor: "billing_meter_owner"},
		{object: reportingObject{kind: reportingObjectRelation, schema: "billing_meter", name: "reservation_hour_collection_current"}, owner: "billing_meter_owner", privilege: reportingPrivilegeSelect, grantee: "k8s_reporting_owner", grantor: "billing_meter_owner"},
		{object: reportingObject{kind: reportingObjectSchema, schema: "billing_meter"}, owner: "billing_meter_owner", privilege: reportingPrivilegeUsage, grantee: "k8s_metabase", grantor: "billing_meter_owner"},
		{object: reportingObject{kind: reportingObjectRelation, schema: "billing_meter", name: "reservation_hour_collection"}, owner: "billing_meter_owner", privilege: reportingPrivilegeSelect, grantee: "k8s_metabase", grantor: "billing_meter_owner"},
		{object: reportingObject{kind: reportingObjectRelation, schema: "billing_meter", name: "reservation_hour_fact"}, owner: "billing_meter_owner", privilege: reportingPrivilegeSelect, grantee: "k8s_metabase", grantor: "billing_meter_owner"},
		{object: reportingObject{kind: reportingObjectRelation, schema: "billing_meter", name: "reservation_hour_current"}, owner: "billing_meter_owner", privilege: reportingPrivilegeSelect, grantee: "k8s_metabase", grantor: "billing_meter_owner"},
		{object: reportingObject{kind: reportingObjectRelation, schema: "billing_meter", name: "reservation_hour_collection_current"}, owner: "billing_meter_owner", privilege: reportingPrivilegeSelect, grantee: "k8s_metabase", grantor: "billing_meter_owner"},
		{object: reportingObject{kind: reportingObjectSchema, schema: "k8s_reporting"}, owner: "k8s_reporting_owner", privilege: reportingPrivilegeUsage, grantee: "k8s_metabase", grantor: "k8s_reporting_owner"},
		{object: reportingObject{kind: reportingObjectRelation, schema: "k8s_reporting", name: "current_resources"}, owner: "k8s_reporting_owner", privilege: reportingPrivilegeSelect, grantee: "k8s_metabase", grantor: "k8s_reporting_owner"},
		{object: reportingObject{kind: reportingObjectRelation, schema: "k8s_reporting", name: "hourly_reservation_usage"}, owner: "k8s_reporting_owner", privilege: reportingPrivilegeSelect, grantee: "k8s_metabase", grantor: "k8s_reporting_owner"},
		{object: reportingObject{kind: reportingObjectRelation, schema: "k8s_reporting", name: "hourly_reservation_usage_excluding_tenants"}, owner: "k8s_reporting_owner", privilege: reportingPrivilegeSelect, grantee: "k8s_metabase", grantor: "k8s_reporting_owner"},
		{object: reportingObject{kind: reportingObjectSchema, schema: "k8s_reporting"}, owner: "k8s_reporting_owner", privilege: reportingPrivilegeUsage, grantee: "cyclops_usage_reader", grantor: "k8s_reporting_owner"},
		{object: reportingObject{kind: reportingObjectRoutine}, routineIdentity: "usage_sandbox_events", owner: "k8s_reporting_owner", privilege: reportingPrivilegeExecute, grantee: "cyclops_usage_reader", grantor: "k8s_reporting_owner"},
		{object: reportingObject{kind: reportingObjectRoutine}, routineIdentity: "reservation_hour_facts", owner: "k8s_reporting_owner", privilege: reportingPrivilegeExecute, grantee: "cyclops_usage_reader", grantor: "k8s_reporting_owner"},
		{object: reportingObject{kind: reportingObjectRoutine}, routineIdentity: "reservation_meter_status", owner: "k8s_reporting_owner", privilege: reportingPrivilegeExecute, grantee: "cyclops_usage_reader", grantor: "k8s_reporting_owner"},
	}
}

func isExpectedReportingACL(acl reportingACL) bool {
	return containsReportingACL(expectedReportingACLs(), acl)
}

func containsReportingACL(acls []reportingACL, want reportingACL) bool {
	for _, acl := range acls {
		objectMatches := acl.object == want.object
		if want.object.kind == reportingObjectRoutine {
			objectMatches = acl.object.kind == reportingObjectRoutine && acl.routineIdentity == want.routineIdentity
		}
		if objectMatches && acl.owner == want.owner && acl.privilege == want.privilege && acl.grantee == want.grantee && acl.grantor == want.grantor && acl.grantable == want.grantable {
			return true
		}
	}
	return false
}

func reconcileUsageRoutineExecute(ctx context.Context, transaction pgx.Tx) error {
	for _, routineIdentity := range []string{"usage_sandbox_events", "reservation_hour_facts", "reservation_meter_status"} {
		var routineOID uint32
		if err := transaction.QueryRow(ctx, `
			select routine.oid
			from pg_proc as routine
			join pg_namespace as namespace on namespace.oid = routine.pronamespace
			where namespace.nspname = 'k8s_reporting'
			  and routine.proname = $1
			  and routine.proargtypes = '25 1184 1184'::oidvector`, routineIdentity).Scan(&routineOID); err != nil {
			return fmt.Errorf("resolve usage reporting routine %s: %w", routineIdentity, err)
		}
		acl := reportingACL{
			object:          reportingObject{kind: reportingObjectRoutine, routineOID: routineOID},
			routineIdentity: routineIdentity,
			owner:           "k8s_reporting_owner",
			privilege:       reportingPrivilegeExecute,
			grantee:         "cyclops_usage_reader",
			grantor:         "k8s_reporting_owner",
		}
		if err := newRuntimeDDL(transaction).grantReportingACL(ctx, acl); err != nil {
			return fmt.Errorf("reconcile usage routine %s execute grant: %w", routineIdentity, err)
		}
	}
	return nil
}

func isSafeReportingACLAuthority(acl reportingACL, migrationOwner string) bool {
	if acl.grantor != acl.owner {
		return false
	}
	return acl.owner == migrationOwner || acl.owner == "k8s_state_owner" || acl.owner == "k8s_reporting_owner" || acl.owner == "billing_meter_owner"
}

func validateNoPublicSecurityDefiner(ctx context.Context, transaction pgx.Tx) error {
	var routine string
	err := transaction.QueryRow(ctx, `
		select routine.oid::regprocedure::text
		from pg_proc as routine
		join pg_namespace as namespace on namespace.oid = routine.pronamespace
		join lateral aclexplode(coalesce(routine.proacl, acldefault('f'::"char", routine.proowner))) as acl on true
		where namespace.nspname !~ '^pg_'
		  and namespace.nspname <> 'information_schema'
		  and routine.prokind in ('f', 'p')
		  and routine.prosecdef
		  and acl.grantee = 0
		  and acl.privilege_type = 'EXECUTE'
		limit 1`).Scan(&routine)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("inspect SECURITY DEFINER routines: %w", err)
	}
	return fmt.Errorf("PUBLIC-executable SECURITY DEFINER routine blocks reporting access: %s", routine)
}

func reconcileStaticRoleContracts(ctx context.Context, transaction pgx.Tx) ([]roleReconciliationEvent, []membershipReconciliationEvent, error) {
	var migrationOwner string
	if err := transaction.QueryRow(ctx, `select current_user`).Scan(&migrationOwner); err != nil {
		return nil, nil, fmt.Errorf("read migration role: %w", err)
	}
	membershipContracts := staticMembershipContracts(migrationOwner)
	membershipEvents, err := reconcileStaticMembershipContracts(ctx, transaction, migrationOwner, membershipContracts)
	if err != nil {
		return nil, nil, err
	}
	if err := validateStaticRoleMemberships(ctx, transaction, migrationOwner, membershipContracts); err != nil {
		return nil, nil, err
	}

	roleEvents := make([]roleReconciliationEvent, 0, len(staticRoleContracts()))
	for _, contract := range staticRoleContracts() {
		started := time.Now()
		attributes, err := readStaticRoleAttributes(ctx, transaction, contract.role)
		if err != nil {
			return nil, nil, err
		}
		if err := newRuntimeDDL(transaction).reconcileStaticRole(ctx, contract, attributes); err != nil {
			return nil, nil, err
		}
		roleEvents = append(roleEvents, roleReconciliationEvent{Role: contract.role, DurationMS: time.Since(started).Milliseconds()})
	}

	if err := reconcileStaticRoleSettings(ctx, transaction); err != nil {
		return nil, nil, err
	}
	return roleEvents, membershipEvents, nil
}

func reconcileStaticMembershipContracts(ctx context.Context, transaction pgx.Tx, migrationOwner string, membershipContracts []staticMembershipContract) ([]membershipReconciliationEvent, error) {
	grantsByContract := make([][]staticMembershipGrant, len(membershipContracts))
	for index, contract := range membershipContracts {
		grants, err := readStaticMembershipGrants(ctx, transaction, contract)
		if err != nil {
			return nil, err
		}
		grantsByContract[index] = grants
	}
	repairs, err := staticMembershipReconciliationPlan(migrationOwner, membershipContracts, grantsByContract)
	if err != nil {
		return nil, err
	}

	membershipEvents := make([]membershipReconciliationEvent, 0, len(membershipContracts))
	var toleratedErrors error
	for index, contract := range membershipContracts {
		started := time.Now()
		if !isStaticMembershipRepair(contract, repairs) {
			membershipEvents = append(membershipEvents, membershipReconciliationEvent{Role: contract.role, DurationMS: time.Since(started).Milliseconds()})
			continue
		}

		authoritativeGrant, err := authoritativeStaticMembershipGrant(contract, migrationOwner, grantsByContract[index])
		if err != nil {
			if !errors.Is(err, errStaticMembershipGrantMissing) && !errors.Is(err, errStaticMembershipGrantOwnerOptions) {
				return nil, errors.Join(err, toleratedErrors)
			}
			toleratedErrors = errors.Join(toleratedErrors, err)
		}

		ddl := newRuntimeDDL(transaction)
		if authoritativeGrant != nil {
			if err := ddl.revokeStaticMembership(ctx, contract, migrationOwner); err != nil {
				return nil, errors.Join(fmt.Errorf("remove drifted static role membership %s -> %s: %w", contract.role, contract.member, err), toleratedErrors)

			}
		}
		if err := ddl.grantStaticMembership(ctx, contract, migrationOwner); err != nil {
			return nil, errors.Join(fmt.Errorf("reconcile static role membership %s -> %s: %w", contract.role, contract.member, err), toleratedErrors)

		}
		membershipEvents = append(membershipEvents, membershipReconciliationEvent{Role: contract.role, DurationMS: time.Since(started).Milliseconds()})
	}
	return membershipEvents, nil
}

func staticMembershipReconciliationPlan(migrationOwner string, membershipContracts []staticMembershipContract, grantsByContract [][]staticMembershipGrant) ([]staticMembershipContract, error) {
	if len(membershipContracts) != len(grantsByContract) {
		return nil, fmt.Errorf("static membership reconciliation plan has %d contracts and %d grant sets", len(membershipContracts), len(grantsByContract))
	}

	repairs := make([]staticMembershipContract, 0, len(membershipContracts))
	for index, contract := range membershipContracts {
		_, err := authoritativeStaticMembershipGrant(contract, migrationOwner, grantsByContract[index])
		if err == nil {
			continue
		}
		if !errors.Is(err, errStaticMembershipGrantMissing) && !errors.Is(err, errStaticMembershipGrantOwnerOptions) {
			return nil, err
		}
		repairs = append(repairs, contract)
	}
	return repairs, nil
}

func isStaticMembershipRepair(contract staticMembershipContract, repairs []staticMembershipContract) bool {
	for _, repair := range repairs {
		if repair == contract {
			return true
		}
	}
	return false
}

func foreignStaticMembershipGrantError(contract staticMembershipContract, grantor, migrationOwner string) error {
	return fmt.Errorf("static role membership %s -> %s has grantor %s instead of migration owner %s; revoke the foreign grant without CASCADE before rerunning the migration", contract.role, contract.member, grantor, migrationOwner)
}

func validateStaticRoleMemberships(ctx context.Context, transaction pgx.Tx, migrationOwner string, contracts []staticMembershipContract) error {
	var toleratedErrors error
	for _, contract := range contracts {
		grants, err := readStaticMembershipGrants(ctx, transaction, contract)
		if err != nil {
			return errors.Join(err, toleratedErrors)
		}
		_, err = authoritativeStaticMembershipGrant(contract, migrationOwner, grants)
		if err != nil {
			if !errors.Is(err, errStaticMembershipGrantMissing) && !errors.Is(err, errStaticMembershipGrantOwnerOptions) {
				return errors.Join(err, toleratedErrors)
			}
			toleratedErrors = errors.Join(toleratedErrors, err)
		}
	}

	dynamicTenants, err := validateQueryTenantRoleMembers(ctx, transaction, migrationOwner, contracts)
	if err != nil {
		return errors.Join(err, toleratedErrors)
	}
	for _, contract := range staticRoleContracts() {
		if contract.role == "k8s_query_tenant" {
			continue
		}
		if err := validateStaticRoleMembers(ctx, transaction, migrationOwner, contract.role, contracts); err != nil {
			return errors.Join(err, toleratedErrors)
		}
	}

	for _, contract := range staticRoleContracts() {
		if err := validateStaticRoleParents(ctx, transaction, contract.role, contracts, dynamicTenants); err != nil {
			return errors.Join(err, toleratedErrors)
		}
	}
	return nil
}

func validateStaticRoleMembers(ctx context.Context, transaction pgx.Tx, migrationOwner, role string, contracts []staticMembershipContract) error {
	creatorGrants, err := readStaticMembershipGrants(ctx, transaction, staticMembershipContract{role: role, member: migrationOwner})
	if err != nil {
		return err
	}
	declaredContract, allowMigrationOwnerGrant := declaredStaticMembershipContract(role, migrationOwner, contracts)
	var declaredGrant *staticMembershipGrant
	if allowMigrationOwnerGrant {
		var err error
		declaredGrant, err = authoritativeStaticMembershipGrant(declaredContract, migrationOwner, creatorGrants)
		if err != nil {
			return err
		}
	}
	if !staticCreatorAdminMembershipsAreExact(migrationOwner, creatorGrants, allowMigrationOwnerGrant, declaredGrant) {
		return staticCreatorAdminMembershipError(role, migrationOwner)
	}

	rows, err := transaction.Query(ctx, `
		select member_role.rolname, grantor_role.rolname, grantor_role.rolsuper, membership.admin_option, membership.inherit_option, membership.set_option
		from pg_auth_members as membership
		join pg_roles as member_role on member_role.oid = membership.member
		join pg_roles as grantor_role on grantor_role.oid = membership.grantor
		where membership.roleid = $1::regrole
		order by member_role.rolname, grantor_role.rolname`, role)
	if err != nil {
		return fmt.Errorf("read static role members for %s: %w", role, err)
	}
	defer rows.Close()

	unexpected := map[string]struct{}{}
	for rows.Next() {
		var member string
		var grant staticMembershipGrant
		if err := rows.Scan(&member, &grant.grantor, &grant.grantorSuperuser, &grant.admin, &grant.inherit, &grant.set); err != nil {
			return fmt.Errorf("scan static role member for %s: %w", role, err)
		}
		if member == migrationOwner || isDeclaredStaticMembership(role, member, contracts) {
			continue
		}
		unexpected[member] = struct{}{}
	}
	if err := rows.Err(); err != nil {
		return fmt.Errorf("iterate static role members for %s: %w", role, err)
	}
	if len(unexpected) != 0 {
		return fmt.Errorf("static role %s has unexpected members %s; revoke them without CASCADE before rerunning the migration", role, sortedRoleNames(unexpected))
	}
	return nil
}

func validateQueryTenantRoleMembers(ctx context.Context, transaction pgx.Tx, migrationOwner string, contracts []staticMembershipContract) (map[string]dynamicTenantRole, error) {
	ddl := newRuntimeDDL(transaction)
	if err := ddl.setLocalRole(ctx, "k8s_state_owner"); err != nil {
		return nil, fmt.Errorf("validate k8s_query_tenant memberships requires SET ROLE k8s_state_owner; restore the declared migration-owner membership before rerunning the migration: %w", err)
	}
	dynamicTenants, err := readRegisteredDynamicTenantRoles(ctx, transaction)
	if err != nil {
		_ = ddl.resetRole(ctx)
		return nil, err
	}
	rows, err := transaction.Query(ctx, `
		select member_role.rolname
		from pg_auth_members as membership
		join pg_roles as member_role on member_role.oid = membership.member
		where membership.roleid = 'k8s_query_tenant'::regrole
		order by member_role.rolname`)
	if err != nil {
		_ = ddl.resetRole(ctx)
		return nil, fmt.Errorf("read k8s_query_tenant members: %w", err)
	}

	unexpected := map[string]struct{}{}
	for rows.Next() {
		var member string
		if err := rows.Scan(&member); err != nil {
			rows.Close()
			_ = ddl.resetRole(ctx)
			return nil, fmt.Errorf("scan k8s_query_tenant member: %w", err)
		}
		if member == migrationOwner || isDeclaredStaticMembership("k8s_query_tenant", member, contracts) {
			continue
		}
		if _, ok := dynamicTenants[member]; ok {
			continue
		}
		unexpected[member] = struct{}{}
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		_ = ddl.resetRole(ctx)
		return nil, fmt.Errorf("iterate k8s_query_tenant members: %w", err)
	}
	rows.Close()
	if err := ddl.resetRole(ctx); err != nil {
		return nil, fmt.Errorf("reset role after validating k8s_query_tenant memberships: %w", err)
	}
	if len(unexpected) != 0 {
		return nil, fmt.Errorf("k8s_query_tenant has unregistered dynamic tenant member %s; remove the membership without CASCADE or register it through k8s_role_admin before rerunning the migration", sortedRoleNames(unexpected))
	}

	creatorGrants, err := readStaticMembershipGrants(ctx, transaction, staticMembershipContract{role: "k8s_query_tenant", member: migrationOwner})
	if err != nil {
		return nil, err
	}
	if !staticCreatorAdminMembershipsAreExact(migrationOwner, creatorGrants, false, nil) {
		return nil, staticCreatorAdminMembershipError("k8s_query_tenant", migrationOwner)
	}
	for _, tenant := range dynamicTenants {
		if err := validateRegisteredTenantRoleMemberships(ctx, transaction, tenant); err != nil {
			return nil, err
		}
	}
	return dynamicTenants, nil
}

func readRegisteredDynamicTenantRoles(ctx context.Context, transaction pgx.Tx) (map[string]dynamicTenantRole, error) {
	rows, err := transaction.Query(ctx, `
		select tenant_role.role_name::text,
			member_role.rolname,
			coalesce(member_role.rolcanlogin, false),
			coalesce(member_role.rolinherit, false),
			coalesce(member_role.rolcreaterole, false),
			coalesce(member_role.rolcreatedb, false),
			coalesce(member_role.rolsuper, false),
			coalesce(member_role.rolreplication, false),
			coalesce(member_role.rolbypassrls, false),
			coalesce(member_role.rolconnlimit, -1),
			coalesce(member_role.rolvaliduntil::text, 'infinity')
		from k8s_state.query_tenant_role as tenant_role
		left join pg_roles as member_role on member_role.rolname = tenant_role.role_name::text
		order by tenant_role.role_name`)
	if err != nil {
		return nil, fmt.Errorf("read registered tenant roles: %w", err)
	}
	defer rows.Close()

	tenants := map[string]dynamicTenantRole{}
	for rows.Next() {
		var tenant dynamicTenantRole
		var roleName *string
		if err := rows.Scan(&tenant.name, &roleName, &tenant.login, &tenant.inherit, &tenant.createRole, &tenant.createDB, &tenant.super, &tenant.replication, &tenant.bypassRLS, &tenant.connectionLimit, &tenant.validUntil); err != nil {
			return nil, fmt.Errorf("scan registered tenant role: %w", err)
		}
		if roleName == nil {
			return nil, fmt.Errorf("registered tenant role %s does not exist; restore or unregister it before rerunning the migration", tenant.name)
		}
		tenant.registered = true
		tenants[tenant.name] = tenant
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate registered tenant roles: %w", err)
	}
	return tenants, nil
}

func validateRegisteredTenantRoleMemberships(ctx context.Context, transaction pgx.Tx, tenant dynamicTenantRole) error {
	creatorGrants, err := readStaticMembershipGrants(ctx, transaction, staticMembershipContract{role: tenant.name, member: "k8s_role_admin"})
	if err != nil {
		return err
	}
	controllerGrants, err := readStaticMembershipGrants(ctx, transaction, staticMembershipContract{role: "k8s_query_tenant", member: tenant.name})
	if err != nil {
		return err
	}
	var inboundCount, parentCount int
	if err := transaction.QueryRow(ctx, `select count(*) from pg_auth_members where roleid = $1::regrole`, tenant.name).Scan(&inboundCount); err != nil {
		return fmt.Errorf("count registered tenant %s members: %w", tenant.name, err)
	}
	if err := transaction.QueryRow(ctx, `select count(*) from pg_auth_members where member = $1::regrole`, tenant.name).Scan(&parentCount); err != nil {
		return fmt.Errorf("count registered tenant %s parent roles: %w", tenant.name, err)
	}
	if inboundCount != 1 || parentCount != 1 || !registeredTenantMembershipsAreExact(tenant, creatorGrants, controllerGrants) {
		return fmt.Errorf("registered tenant role %s has memberships outside the controller contract; remove the drift without CASCADE before rerunning the migration", tenant.name)
	}
	return nil
}

func staticCreatorAdminMembershipsAreExact(member string, grants []staticMembershipGrant, allowOwnerGrant bool, declaredGrant *staticMembershipGrant) bool {
	foreignCount := 0
	ownerCount := 0
	for _, grant := range grants {
		if declaredGrant != nil && grant == *declaredGrant {
			declaredGrant = nil
			continue
		}
		if grant.grantor == member {
			ownerCount++
			continue
		}
		foreignCount++
		if !grant.grantorSuperuser || !grant.admin || grant.inherit || grant.set {
			return false
		}
	}
	return foreignCount <= 1 && ownerCount <= 1 && (allowOwnerGrant || ownerCount == 0)
}

func staticCreatorAdminMembershipError(role, member string) error {
	return fmt.Errorf("role %s must have at most one foreign creator-admin membership for %s granted by a true PostgreSQL superuser with admin=true inherit=false set=false", role, member)
}

func creatorAdminMembershipsAreExact(member string, grants []staticMembershipGrant) bool {
	return len(grants) == 1 &&
		grants[0].grantor != member &&
		grants[0].grantorSuperuser &&
		grants[0].admin && !grants[0].inherit && !grants[0].set
}

func registeredTenantMembershipsAreExact(tenant dynamicTenantRole, inbound, parents []staticMembershipGrant) bool {
	return dynamicTenantRoleMatchesControllerContract(tenant) &&
		creatorAdminMembershipsAreExact("k8s_role_admin", inbound) &&
		len(parents) == 1 &&
		parents[0].grantor == "k8s_role_admin" &&
		!parents[0].admin && parents[0].inherit && !parents[0].set
}

func creatorAdminMembershipError(role, member string) error {
	return fmt.Errorf("role %s must have exactly one implicit creator-admin membership for %s granted by a true PostgreSQL superuser with admin=true inherit=false set=false", role, member)
}

func dynamicTenantRoleMatchesControllerContract(tenant dynamicTenantRole) bool {
	return tenant.registered &&
		len(tenant.name) == len("k8s_tenant_")+32 &&
		strings.HasPrefix(tenant.name, "k8s_tenant_") &&
		isLowerHex(tenant.name[len("k8s_tenant_"):]) &&
		tenant.login && tenant.inherit && !tenant.createRole && !tenant.createDB && !tenant.super && !tenant.replication && !tenant.bypassRLS &&
		tenant.connectionLimit == -1 && tenant.validUntil == "infinity"
}

func allowsRegisteredTenantCreatorAdminMembership(tenant dynamicTenantRole, member string, grant staticMembershipGrant) bool {
	return dynamicTenantRoleMatchesControllerContract(tenant) &&
		member == "k8s_role_admin" &&
		grant.grantor != member &&
		grant.grantorSuperuser &&
		grant.admin && !grant.inherit && !grant.set
}

func declaredStaticMembershipContract(role, member string, contracts []staticMembershipContract) (staticMembershipContract, bool) {
	for _, contract := range contracts {
		if contract.role == role && contract.member == member {
			return contract, true
		}
	}
	return staticMembershipContract{}, false
}

func isDeclaredStaticMembership(role, member string, contracts []staticMembershipContract) bool {
	_, ok := declaredStaticMembershipContract(role, member, contracts)
	return ok
}

func isLowerHex(value string) bool {
	for _, character := range value {
		if (character < '0' || character > '9') && (character < 'a' || character > 'f') {
			return false
		}
	}
	return true
}

func validateStaticRoleParents(ctx context.Context, transaction pgx.Tx, role string, contracts []staticMembershipContract, dynamicTenants map[string]dynamicTenantRole) error {
	rows, err := transaction.Query(ctx, `
		select parent_role.rolname, grantor_role.rolname, grantor_role.rolsuper, membership.admin_option, membership.inherit_option, membership.set_option
		from pg_auth_members as membership
		join pg_roles as parent_role on parent_role.oid = membership.roleid
		join pg_roles as grantor_role on grantor_role.oid = membership.grantor
		where membership.member = $1::regrole
		order by parent_role.rolname`, role)
	if err != nil {
		return fmt.Errorf("read static role parents for %s: %w", role, err)
	}
	defer rows.Close()

	unexpected := map[string]struct{}{}
	for rows.Next() {
		var parent string
		var grant staticMembershipGrant
		if err := rows.Scan(&parent, &grant.grantor, &grant.grantorSuperuser, &grant.admin, &grant.inherit, &grant.set); err != nil {
			return fmt.Errorf("scan static role parent for %s: %w", role, err)
		}
		if isDeclaredStaticMembership(parent, role, contracts) || allowsRegisteredTenantCreatorAdminMembership(dynamicTenants[parent], role, grant) {
			continue
		}
		unexpected[parent] = struct{}{}
	}
	if err := rows.Err(); err != nil {
		return fmt.Errorf("iterate static role parents for %s: %w", role, err)
	}
	if len(unexpected) != 0 {
		return fmt.Errorf("static role %s has unexpected parent roles %s; revoke them without CASCADE before rerunning the migration", role, sortedRoleNames(unexpected))
	}
	return nil
}

func sortedRoleNames(names map[string]struct{}) string {
	ordered := make([]string, 0, len(names))
	for name := range names {
		ordered = append(ordered, name)
	}
	sort.Strings(ordered)
	return strings.Join(ordered, ", ")
}

func reconcileStaticRoleSettings(ctx context.Context, transaction pgx.Tx) error {
	contracts := staticRoleSettingsContracts()
	for _, roleContract := range staticRoleContracts() {
		actual, err := readStaticRoleSettings(ctx, transaction, roleContract.role)
		if err != nil {
			return err
		}
		desired := contracts[roleContract.role]
		if staticRoleSettingsMatch(actual, desired) {
			continue
		}

		if err := newRuntimeDDL(transaction).reconcileStaticRoleSettings(ctx, roleContract, actual, desired); err != nil {
			return err
		}
	}
	return nil
}

type staticRoleSetting struct {
	database   string
	values     staticRoleSettingsContract
	hasUnknown bool
}

func readStaticRoleSettings(ctx context.Context, transaction pgx.Tx, role string) ([]staticRoleSetting, error) {
	rows, err := transaction.Query(ctx, `
		select coalesce(database.datname, ''), setting.setconfig
		from pg_db_role_setting as setting
		join pg_roles as configured_role on configured_role.oid = setting.setrole
		left join pg_database as database on database.oid = setting.setdatabase
		where configured_role.rolname = $1
		order by setting.setdatabase`, role)
	if err != nil {
		return nil, fmt.Errorf("read static role settings for %s: %w", role, err)
	}
	defer rows.Close()

	settings := []staticRoleSetting{}
	for rows.Next() {
		var setting staticRoleSetting
		var values []string
		if err := rows.Scan(&setting.database, &values); err != nil {
			return nil, fmt.Errorf("scan static role settings for %s: %w", role, err)
		}
		setting.values = make(staticRoleSettingsContract, len(values))
		for _, value := range values {
			sqlName, configuredValue, ok := strings.Cut(value, "=")
			if !ok {
				return nil, fmt.Errorf("static role %s has malformed setting %q", role, value)
			}
			name, approved := staticRoleSettingNameFromSQL(sqlName)
			if !approved {
				setting.hasUnknown = true
				continue
			}
			setting.values[name] = configuredValue
		}
		settings = append(settings, setting)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate static role settings for %s: %w", role, err)
	}
	return settings, nil
}

func staticRoleSettingsMatch(actual []staticRoleSetting, desired staticRoleSettingsContract) bool {
	if len(desired) == 0 {
		return len(actual) == 0
	}
	if len(actual) != 1 || actual[0].database != "" || actual[0].hasUnknown || len(actual[0].values) != len(desired) {
		return false
	}
	for name, desiredValue := range desired {
		if actual[0].values[name] != desiredValue {
			return false
		}
	}
	return true
}

func isMigrationOwnedStaticRole(role string) bool {
	for _, contract := range staticRoleContracts() {
		if contract.role == role {
			return true
		}
	}
	return false
}

func allowsImplicitCreatorAdminMembership(role, member, migrationOwner string, grant staticMembershipGrant) bool {
	return isMigrationOwnedStaticRole(role) &&
		member == migrationOwner &&
		grant.grantor != migrationOwner &&
		grant.grantorSuperuser &&
		grant.admin &&
		!grant.inherit &&
		!grant.set
}

var (
	errStaticMembershipGrantMissing      = errors.New("static membership grant missing")
	errStaticMembershipGrantOwnerOptions = errors.New("static membership grant owner options drifted")
)

func authoritativeStaticMembershipGrant(contract staticMembershipContract, migrationOwner string, grants []staticMembershipGrant) (*staticMembershipGrant, error) {
	nonImplicitGrants := make([]staticMembershipGrant, 0, len(grants))
	for _, grant := range grants {
		if !allowsImplicitCreatorAdminMembership(contract.role, contract.member, migrationOwner, grant) {
			nonImplicitGrants = append(nonImplicitGrants, grant)
		}
	}
	if len(nonImplicitGrants) == 0 {
		return nil, fmt.Errorf("static role membership %s -> %s: %w", contract.role, contract.member, errStaticMembershipGrantMissing)
	}
	for _, grant := range nonImplicitGrants {
		if grant.grantor != migrationOwner && !grant.grantorSuperuser {
			return nil, foreignStaticMembershipGrantError(contract, grant.grantor, migrationOwner)
		}
	}
	if len(nonImplicitGrants) != 1 {
		return nil, fmt.Errorf("static role membership %s -> %s has %d non-implicit grants; revoke duplicate grants without CASCADE before rerunning the migration", contract.role, contract.member, len(nonImplicitGrants))
	}

	grant := nonImplicitGrants[0]
	if grant.admin != contract.admin || grant.inherit != contract.inherit || grant.set != contract.set {
		if grant.grantor == migrationOwner {
			return &grant, fmt.Errorf("static role membership %s -> %s: %w", contract.role, contract.member, errStaticMembershipGrantOwnerOptions)
		}
		return nil, fmt.Errorf("static role membership %s -> %s has options that do not match its contract", contract.role, contract.member)
	}
	return &grant, nil
}

func readStaticMembershipGrants(ctx context.Context, transaction pgx.Tx, contract staticMembershipContract) ([]staticMembershipGrant, error) {
	rows, err := transaction.Query(ctx, `
		select grantor_role.rolname, grantor_role.rolsuper, membership.admin_option, membership.inherit_option, membership.set_option
		from pg_auth_members as membership
		join pg_roles as grantor_role on grantor_role.oid = membership.grantor
		where membership.roleid = $1::regrole and membership.member = $2::regrole
		order by grantor_role.rolname`, contract.role, contract.member)
	if err != nil {
		return nil, fmt.Errorf("read static role membership %s -> %s: %w", contract.role, contract.member, err)
	}
	defer rows.Close()

	var grants []staticMembershipGrant
	for rows.Next() {
		var grant staticMembershipGrant
		if err := rows.Scan(&grant.grantor, &grant.grantorSuperuser, &grant.admin, &grant.inherit, &grant.set); err != nil {
			return nil, fmt.Errorf("scan static role membership %s -> %s: %w", contract.role, contract.member, err)
		}
		grants = append(grants, grant)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate static role membership %s -> %s: %w", contract.role, contract.member, err)
	}
	return grants, nil
}

func reconcilePasswords(ctx context.Context, transaction pgx.Tx, credentials []credential) ([]credentialEvent, error) {
	events := make([]credentialEvent, 0, len(credentials))
	for _, credential := range credentials {
		started := time.Now()
		if err := newRuntimeDDL(transaction).setPassword(ctx, credential); err != nil {
			return nil, fmt.Errorf("reconcile password for role %s: %w", credential.Role, err)
		}
		events = append(events, credentialEvent{Role: credential.Role, DurationMS: time.Since(started).Milliseconds()})
	}
	return events, nil
}

func CurrentVersion(ctx context.Context, url string) (int64, error) {
	connectionConfig, err := pgx.ParseConfig(url)
	if err != nil {
		return 0, fmt.Errorf("%w: parse database URL: %w", ErrInvalidConfiguration, err)

	}
	connection, err := pgx.ConnectConfig(ctx, connectionConfig)
	if err != nil {
		return 0, fmt.Errorf("%w%s: %w", ErrUnavailable, databaseTarget(connectionConfig), err)

	}
	defer connection.Close(ctx)

	var version int64
	err = connection.QueryRow(ctx, `select coalesce(max(version), 0) from cyclops_migrations.applied_migrations`).Scan(&version)
	if err != nil {
		var databaseError *pgconn.PgError
		if errors.As(err, &databaseError) && (databaseError.Code == "42P01" || databaseError.Code == "3F000") {
			return 0, nil
		}
		return 0, fmt.Errorf("read database schema version: %w", err)
	}
	return version, nil
}

func RequireVersion(ctx context.Context, url string, minimum int64) error {
	current, err := CurrentVersion(ctx, url)
	if err != nil {
		return err
	}
	if current < minimum {
		return fmt.Errorf("database schema version %d is older than required version %d", current, minimum)
	}
	return nil
}
