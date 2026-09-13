package database

import (
	"context"
	"fmt"
	"sort"
	"strconv"
	"strings"

	"github.com/jackc/pgx/v5"
)

const hourlyReservationMeterOriginalSHA256 = "fabda9a67ab9323b70a6d189a89be4194e1a76778a8c3afc77a0f1763f82bf4e"

const hourlyReservationMeterPrivilegeSequence = `CREATE SCHEMA billing_meter AUTHORIZATION billing_meter_owner;
REVOKE CREATE ON SCHEMA billing_meter FROM PUBLIC;

SET LOCAL ROLE billing_meter_owner;`

const hourlyReservationMeterCompatiblePrivilegeSequence = `CREATE SCHEMA billing_meter AUTHORIZATION billing_meter_owner;
SET LOCAL ROLE billing_meter_owner;
REVOKE CREATE ON SCHEMA billing_meter FROM PUBLIC;`

const createAppliedMigrationsTableStatement = `create table if not exists cyclops_migrations.applied_migrations (
	application_order bigint generated always as identity unique,
	version bigint primary key,
	filename text not null unique,
	sha256 text not null,
	applied_at timestamptz not null default clock_timestamp()
)`

type staticRoleValidUntil uint8

const staticRoleValidUntilInfinity staticRoleValidUntil = 1

type staticRoleSettingName uint8

const (
	staticRoleSettingDefaultTransactionReadOnly staticRoleSettingName = iota + 1
	staticRoleSettingStatementTimeout
	staticRoleSettingIdleInTransactionSessionTimeout
)

var staticRoleSettingSQLNames = map[staticRoleSettingName]string{
	staticRoleSettingDefaultTransactionReadOnly:      "default_transaction_read_only",
	staticRoleSettingStatementTimeout:                "statement_timeout",
	staticRoleSettingIdleInTransactionSessionTimeout: "idle_in_transaction_session_timeout",
}

type staticRoleSettingsContract map[staticRoleSettingName]string

type reportingObjectKind uint8

const (
	reportingObjectSchema reportingObjectKind = iota + 1
	reportingObjectRelation
	reportingObjectSequence
	reportingObjectRoutine
)

type reportingPrivilege uint8

const (
	reportingPrivilegeUsage reportingPrivilege = iota + 1
	reportingPrivilegeCreate
	reportingPrivilegeSelect
	reportingPrivilegeInsert
	reportingPrivilegeUpdate
	reportingPrivilegeDelete
	reportingPrivilegeTruncate
	reportingPrivilegeReferences
	reportingPrivilegeTrigger
	reportingPrivilegeExecute
)

var reportingPrivilegeSQLNames = map[reportingPrivilege]string{
	reportingPrivilegeUsage:      "USAGE",
	reportingPrivilegeCreate:     "CREATE",
	reportingPrivilegeSelect:     "SELECT",
	reportingPrivilegeInsert:     "INSERT",
	reportingPrivilegeUpdate:     "UPDATE",
	reportingPrivilegeDelete:     "DELETE",
	reportingPrivilegeTruncate:   "TRUNCATE",
	reportingPrivilegeReferences: "REFERENCES",
	reportingPrivilegeTrigger:    "TRIGGER",
	reportingPrivilegeExecute:    "EXECUTE",
}

var reportingPrivilegesByKind = map[reportingObjectKind]map[reportingPrivilege]struct{}{
	reportingObjectSchema: {
		reportingPrivilegeUsage:  {},
		reportingPrivilegeCreate: {},
	},
	reportingObjectRelation: {
		reportingPrivilegeSelect:     {},
		reportingPrivilegeInsert:     {},
		reportingPrivilegeUpdate:     {},
		reportingPrivilegeDelete:     {},
		reportingPrivilegeTruncate:   {},
		reportingPrivilegeReferences: {},
		reportingPrivilegeTrigger:    {},
	},
	reportingObjectSequence: {
		reportingPrivilegeUsage:  {},
		reportingPrivilegeSelect: {},
		reportingPrivilegeUpdate: {},
	},
	reportingObjectRoutine: {
		reportingPrivilegeExecute: {},
	},
}

type reportingObject struct {
	kind       reportingObjectKind
	schema     string
	name       string
	routineOID uint32
}

type runtimeDDL struct {
	transaction pgx.Tx
}

func (name staticRoleSettingName) sqlName() string {
	return staticRoleSettingSQLNames[name]
}

func staticRoleSettingNameFromSQL(sqlName string) (staticRoleSettingName, bool) {
	for name, approvedSQLName := range staticRoleSettingSQLNames {
		if sqlName == approvedSQLName {
			return name, true
		}
	}
	return 0, false
}

func reportingObjectKindFromSQL(sqlName string) (reportingObjectKind, bool) {
	switch sqlName {
	case "schema":
		return reportingObjectSchema, true
	case "relation":
		return reportingObjectRelation, true
	case "sequence":
		return reportingObjectSequence, true
	case "routine":
		return reportingObjectRoutine, true
	default:
		return 0, false
	}
}

func (kind reportingObjectKind) sqlName() string {
	switch kind {
	case reportingObjectSchema:
		return "schema"
	case reportingObjectRelation:
		return "table"
	case reportingObjectSequence:
		return "sequence"
	case reportingObjectRoutine:
		return "routine"
	default:
		return ""
	}
}

func reportingPrivilegeFromSQL(kind reportingObjectKind, sqlName string) (reportingPrivilege, bool) {
	for privilege := range reportingPrivilegesByKind[kind] {
		if reportingPrivilegeSQLNames[privilege] == sqlName {
			return privilege, true
		}
	}
	return 0, false
}

func (privilege reportingPrivilege) sqlName() string {
	return reportingPrivilegeSQLNames[privilege]
}

func reportingObjectSQLName(object reportingObject) (string, error) {
	switch object.kind {
	case reportingObjectSchema:
		if object.schema == "" || object.name != "" || object.routineOID != 0 {
			return "", errorsInvalidReportingObject(object)
		}
		return pgx.Identifier{object.schema}.Sanitize(), nil
	case reportingObjectRelation, reportingObjectSequence:
		if object.schema == "" || object.name == "" || object.routineOID != 0 {
			return "", errorsInvalidReportingObject(object)
		}
		return pgx.Identifier{object.schema, object.name}.Sanitize(), nil
	case reportingObjectRoutine:
		return "", fmt.Errorf("reporting routine OID must be canonicalized through PostgreSQL")
	default:
		return "", errorsInvalidReportingObject(object)
	}
}

func errorsInvalidReportingObject(object reportingObject) error {
	return fmt.Errorf("invalid reporting object identity for kind %d", object.kind)
}

func canonicalReportingObjectSQL(ctx context.Context, transaction pgx.Tx, object reportingObject) (string, error) {
	if object.kind != reportingObjectRoutine {
		return reportingObjectSQLName(object)
	}
	if object.schema != "" || object.name != "" || object.routineOID == 0 {
		return "", errorsInvalidReportingObject(object)
	}
	var canonical string
	if err := transaction.QueryRow(ctx, `select $1::oid::regprocedure::text`, object.routineOID).Scan(&canonical); err != nil {
		return "", fmt.Errorf("canonicalize reporting routine OID %d: %w", object.routineOID, err)
	}
	return canonical, nil
}

func reportingObjectDescription(object reportingObject) string {
	if object.kind == reportingObjectRoutine {
		return fmt.Sprintf("routine oid %d", object.routineOID)
	}
	name, err := reportingObjectSQLName(object)
	if err != nil {
		return fmt.Sprintf("invalid kind %d", object.kind)
	}
	return object.kind.sqlName() + " " + name
}

func postgresBoolean(value bool) string {
	if value {
		return "TRUE"
	}
	return "FALSE"
}

func staticRoleAlterClauses(contract staticRoleContract, attributes staticRoleAttributes) (string, error) {
	unsafeAttributes := make([]string, 0, 3)
	if attributes.super {
		unsafeAttributes = append(unsafeAttributes, "rolsuper=true")
	}
	if attributes.replication {
		unsafeAttributes = append(unsafeAttributes, "rolreplication=true")
	}
	if attributes.bypassRLS {
		unsafeAttributes = append(unsafeAttributes, "rolbypassrls=true")
	}
	if len(unsafeAttributes) != 0 {
		return "", fmt.Errorf("static role %s has unsafe privileged drift: %s; the migration owner cannot safely repair these attributes", contract.role, strings.Join(unsafeAttributes, ", "))
	}
	if contract.createDB {
		return "", fmt.Errorf("static role %s has unsupported CREATEDB contract; the migrator only reconciles roles to NOCREATEDB", contract.role)
	}
	if contract.validUntil != staticRoleValidUntilInfinity {
		return "", fmt.Errorf("static role %s has unsupported valid-until contract %d; only infinity is supported", contract.role, contract.validUntil)
	}

	clauses := make([]string, 0, 7)
	if attributes.login != contract.login {
		clauses = append(clauses, map[bool]string{true: "LOGIN", false: "NOLOGIN"}[contract.login])
	}
	if attributes.inherit != contract.inherit {
		clauses = append(clauses, map[bool]string{true: "INHERIT", false: "NOINHERIT"}[contract.inherit])
	}
	if attributes.createRole != contract.createRole {
		clauses = append(clauses, map[bool]string{true: "CREATEROLE", false: "NOCREATEROLE"}[contract.createRole])
	}
	if attributes.createDB {
		clauses = append(clauses, "NOCREATEDB")
	}
	if attributes.connectionLimit != contract.connectionLimit {
		clauses = append(clauses, "CONNECTION LIMIT "+strconv.Itoa(contract.connectionLimit))
	}
	if attributes.validUntil != "infinity" {
		clauses = append(clauses, "VALID UNTIL 'infinity'")
	}
	return strings.Join(clauses, " "), nil
}

func prepareMigrationExecution(file migrationFile) (migrationFile, error) {
	if file.SHA256 != hourlyReservationMeterOriginalSHA256 {
		return file, nil
	}
	if file.Name != "000005_hourly_reservation_meter.sql" {
		return migrationFile{}, fmt.Errorf("legacy reservation meter checksum belongs to unexpected migration %s", file.Name)
	}
	if strings.Count(file.SQL, hourlyReservationMeterPrivilegeSequence) != 1 {
		return migrationFile{}, fmt.Errorf("legacy reservation meter privilege sequence is missing or ambiguous")
	}
	file.SQL = strings.Replace(file.SQL, hourlyReservationMeterPrivilegeSequence, hourlyReservationMeterCompatiblePrivilegeSequence, 1)
	return file, nil
}

func newRuntimeDDL(transaction pgx.Tx) runtimeDDL {
	return runtimeDDL{transaction: transaction}
}

func (ddl runtimeDDL) ensureMigrationLedger(ctx context.Context) error {
	for _, statement := range []string{
		`select pg_advisory_xact_lock(hashtext('cyclops-database-migrations'))`,
		`create schema if not exists cyclops_migrations`,
		createAppliedMigrationsTableStatement,
	} {
		if _, err := ddl.transaction.Exec(ctx, statement); err != nil {
			return err
		}
	}
	return nil
}

func (ddl runtimeDDL) reconcileStaticRole(ctx context.Context, contract staticRoleContract, attributes staticRoleAttributes) error {
	clauses, err := staticRoleAlterClauses(contract, attributes)
	if err != nil || clauses == "" {
		return err
	}
	if _, err := ddl.transaction.Exec(ctx, "alter role "+pgx.Identifier{contract.role}.Sanitize()+" "+clauses); err != nil {
		return fmt.Errorf("reconcile static role %s: %w", contract.role, err)
	}
	return nil
}

func (ddl runtimeDDL) revokeStaticMembership(ctx context.Context, contract staticMembershipContract, grantor string) error {
	_, err := ddl.transaction.Exec(ctx,
		"revoke "+pgx.Identifier{contract.role}.Sanitize()+
			" from "+pgx.Identifier{contract.member}.Sanitize()+
			" granted by "+pgx.Identifier{grantor}.Sanitize()+" restrict",
	)
	return err
}

func (ddl runtimeDDL) grantStaticMembership(ctx context.Context, contract staticMembershipContract, grantor string) error {
	_, err := ddl.transaction.Exec(ctx, staticMembershipGrantStatement(contract, grantor))
	return err
}

func staticMembershipGrantStatement(contract staticMembershipContract, grantor string) string {
	statement := "grant " + pgx.Identifier{contract.role}.Sanitize() +
		" to " + pgx.Identifier{contract.member}.Sanitize() +
		" with admin " + postgresBoolean(contract.admin) +
		", inherit " + postgresBoolean(contract.inherit) +
		", set " + postgresBoolean(contract.set)
	if !contract.admin {
		statement += " granted by " + pgx.Identifier{grantor}.Sanitize()
	}
	return statement
}

func (ddl runtimeDDL) reconcileStaticRoleSettings(ctx context.Context, role staticRoleContract, actual []staticRoleSetting, desired staticRoleSettingsContract) error {
	if staticRoleSettingsMatch(actual, desired) {
		return nil
	}

	roleIdentifier := pgx.Identifier{role.role}.Sanitize()
	for _, setting := range actual {
		statement := "alter role " + roleIdentifier
		if setting.database != "" {
			statement += " in database " + pgx.Identifier{setting.database}.Sanitize()
		}
		if _, err := ddl.transaction.Exec(ctx, statement+" reset all"); err != nil {
			return fmt.Errorf("reset static role settings for %s: %w", role.role, err)
		}
	}

	settingNames := make([]staticRoleSettingName, 0, len(desired))
	for name := range desired {
		if name.sqlName() == "" {
			return fmt.Errorf("unsupported static role setting %d for %s", name, role.role)
		}
		settingNames = append(settingNames, name)
	}
	sort.Slice(settingNames, func(left, right int) bool { return settingNames[left] < settingNames[right] })
	for _, name := range settingNames {
		var value string
		if err := ddl.transaction.QueryRow(ctx, `select format('%L', $1::text)`, desired[name]).Scan(&value); err != nil {
			return fmt.Errorf("quote static role setting %s for %s: %w", name.sqlName(), role.role, err)
		}
		if _, err := ddl.transaction.Exec(ctx, "alter role "+roleIdentifier+" set "+name.sqlName()+" = "+value); err != nil {
			return fmt.Errorf("reconcile static role setting %s for %s: %w", name.sqlName(), role.role, err)
		}
	}
	return nil
}

func (ddl runtimeDDL) setPassword(ctx context.Context, credential credential) error {
	var quotedPassword string
	if err := ddl.transaction.QueryRow(ctx, `select format('%L', $1::text)`, credential.Password).Scan(&quotedPassword); err != nil {
		return fmt.Errorf("quote password for role %s: %w", credential.Role, err)
	}
	_, err := ddl.transaction.Exec(ctx, "alter role "+pgx.Identifier{credential.Role}.Sanitize()+" password "+quotedPassword)
	return err
}

func (ddl runtimeDDL) setLocalRole(ctx context.Context, role string) error {
	_, err := ddl.transaction.Exec(ctx, "set local role "+pgx.Identifier{role}.Sanitize())
	return err
}

func (ddl runtimeDDL) resetRole(ctx context.Context) error {
	_, err := ddl.transaction.Exec(ctx, "reset role")
	return err
}

func (ddl runtimeDDL) grantReportingACL(ctx context.Context, acl reportingACL) (runErr error) {
	if !isExpectedReportingACL(acl) {
		return fmt.Errorf("unsupported reporting ACL grant on %s", reportingObjectDescription(acl.object))
	}
	objectName, err := canonicalReportingObjectSQL(ctx, ddl.transaction, acl.object)
	if err != nil {
		return err
	}
	if err := ddl.setLocalRole(ctx, acl.owner); err != nil {
		return err
	}
	defer func() {
		if err := ddl.resetRole(ctx); err != nil && runErr == nil {
			runErr = err
		}
	}()

	statement := "grant " + acl.privilege.sqlName() + " on " + acl.object.kind.sqlName() + " " + objectName + " to " + pgx.Identifier{acl.grantee}.Sanitize()
	_, runErr = ddl.transaction.Exec(ctx, statement)
	return runErr
}

func (ddl runtimeDDL) revokeReportingACL(ctx context.Context, acl reportingACL) (runErr error) {
	if _, ok := reportingPrivilegesByKind[acl.object.kind][acl.privilege]; !ok {
		return fmt.Errorf("unsupported reporting privilege for %s", reportingObjectDescription(acl.object))
	}
	objectName, err := canonicalReportingObjectSQL(ctx, ddl.transaction, acl.object)
	if err != nil {
		return err
	}
	if err := ddl.setLocalRole(ctx, acl.owner); err != nil {
		return err
	}
	defer func() {
		if err := ddl.resetRole(ctx); err != nil && runErr == nil {
			runErr = err
		}
	}()

	grantee := pgx.Identifier{acl.grantee}.Sanitize()
	if acl.grantee == "PUBLIC" {
		grantee = "public"
	}
	statement := "revoke " + acl.privilege.sqlName() + " on " + acl.object.kind.sqlName() + " " + objectName + " from " + grantee + " granted by " + pgx.Identifier{acl.owner}.Sanitize()
	_, runErr = ddl.transaction.Exec(ctx, statement)
	return runErr
}
