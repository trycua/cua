package database

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"log/slog"
	"os"
	"reflect"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

func TestBackendImageBuildsMigrationCommand(t *testing.T) {
	contents, err := os.ReadFile("../Dockerfile")
	if err != nil {
		t.Fatal(err)
	}

	dockerfile := strings.ReplaceAll(string(contents), "\\\n", " ")
	var buildsMigrator, copiesMigrator bool
	for _, line := range strings.Split(dockerfile, "\n") {
		fields := strings.Fields(strings.TrimSpace(line))
		if len(fields) == 0 {
			continue
		}

		switch fields[0] {
		case "RUN":
			for _, command := range strings.Split(strings.Join(fields[1:], " "), "&&") {
				command = strings.NewReplacer("\"", "", "'", "").Replace(command)
				buildsMigrator = buildsMigrator ||
					strings.Contains(command, "CGO_ENABLED=1") &&
						strings.Contains(command, "GOOS=linux") &&
						strings.Contains(command, "go build") &&
						strings.Contains(command, "-trimpath") &&
						strings.Contains(command, "-ldflags=") &&
						strings.Contains(command, "-o /out/cyclops-db-migrate") &&
						strings.Contains(command, "./cmd/db-migrate")
			}
		case "COPY":
			copiesMigrator = copiesMigrator ||
				len(fields) == 4 &&
					fields[1] == "--from=go-builder" &&
					fields[2] == "/out/cyclops-db-migrate" &&
					fields[3] == "/app/cyclops-db-migrate"
		}
	}

	if !buildsMigrator {
		t.Fatal("Dockerfile does not build /out/cyclops-db-migrate with the backend build flags")
	}
	if !copiesMigrator {
		t.Fatal("Dockerfile does not copy /app/cyclops-db-migrate from go-builder")
	}
}

func TestTenantCredentialFingerprintLookupIsParameterized(t *testing.T) {
	const expected = `select credential_fingerprint from k8s_state.query_tenant_role where role_name = $1`
	if tenantCredentialFingerprintLookup != expected {
		t.Fatalf("tenant credential fingerprint lookup = %q, want %q", tenantCredentialFingerprintLookup, expected)
	}
}

func TestStaticRoleAlterClausesOnlyContainPermittedDrift(t *testing.T) {
	contract := staticRoleContract{role: "cyclops_app", login: true, connectionLimit: -1, validUntil: staticRoleValidUntilInfinity}
	clauses, err := staticRoleAlterClauses(contract, staticRoleAttributes{
		login:           false,
		inherit:         true,
		createRole:      true,
		connectionLimit: 1,
		validUntil:      "2026-08-10 00:00:00+00",
	})
	if err != nil {
		t.Fatal(err)
	}
	if clauses != "LOGIN NOINHERIT NOCREATEROLE CONNECTION LIMIT -1 VALID UNTIL 'infinity'" {
		t.Fatalf("static role alter clauses = %q", clauses)
	}
	if strings.Contains(clauses, "SUPERUSER") || strings.Contains(clauses, "REPLICATION") || strings.Contains(clauses, "BYPASSRLS") {
		t.Fatalf("static role alter clauses contain a superuser-only attribute: %q", clauses)
	}
}

func TestStaticRoleSettingsContractsAreExact(t *testing.T) {
	want := map[string]staticRoleSettingsContract{
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
	if !reflect.DeepEqual(staticRoleSettingsContracts(), want) {
		t.Fatalf("static role settings contracts = %#v, want %#v", staticRoleSettingsContracts(), want)
	}
}

func TestStaticRoleAlterClausesRejectUnsupportedValidUntilContract(t *testing.T) {
	_, err := staticRoleAlterClauses(
		staticRoleContract{role: "cyclops_app", login: true, validUntil: staticRoleValidUntil(255)},
		staticRoleAttributes{login: true, validUntil: "2026-08-10 00:00:00+00"},
	)
	if err == nil || !strings.Contains(err.Error(), "unsupported valid-until contract") {
		t.Fatalf("static role valid-until error = %v, want closed infinity-only contract", err)
	}
}

func TestStaticRoleAlterClausesReconcileCreateDBDrift(t *testing.T) {
	clauses, err := staticRoleAlterClauses(staticRoleContract{role: "cyclops_app", login: true, validUntil: staticRoleValidUntilInfinity}, staticRoleAttributes{
		login:      true,
		createDB:   true,
		validUntil: "infinity",
	})
	if err != nil {
		t.Fatal(err)
	}
	if clauses != "NOCREATEDB" {
		t.Fatalf("static role createdb clauses = %q, want NOCREATEDB", clauses)
	}
}

func TestStaticRoleAlterClausesRejectCreateDBContract(t *testing.T) {
	_, err := staticRoleAlterClauses(
		staticRoleContract{role: "cyclops_app", login: true, createDB: true, validUntil: staticRoleValidUntilInfinity},
		staticRoleAttributes{login: true, validUntil: "infinity"},
	)
	if err == nil || !strings.Contains(err.Error(), "unsupported CREATEDB contract") {
		t.Fatalf("static role CREATEDB contract error = %v, want fail-closed unsupported contract", err)
	}
}

func TestStaticRoleAlterClausesRejectUnsafeSuperuserOnlyDrift(t *testing.T) {
	_, err := staticRoleAlterClauses(staticRoleContract{role: "cyclops_app", login: true}, staticRoleAttributes{
		login:       true,
		super:       true,
		replication: true,
		bypassRLS:   true,
	})
	if err == nil {
		t.Fatal("expected unsafe superuser-only role drift to fail closed")
	}
	for _, attribute := range []string{"rolsuper", "rolreplication", "rolbypassrls"} {
		if !strings.Contains(err.Error(), attribute) {
			t.Errorf("unsafe role drift error = %q, want %s", err, attribute)
		}
	}
}

func TestAllowsRegisteredTenantCreatorAdminMembership(t *testing.T) {
	validTenant := dynamicTenantRole{
		name:            "k8s_tenant_0123456789abcdef0123456789abcdef",
		registered:      true,
		login:           true,
		inherit:         true,
		connectionLimit: -1,
		validUntil:      "infinity",
	}
	grant := staticMembershipGrant{grantor: "bootstrap_owner", grantorSuperuser: true, admin: true, inherit: false, set: false}
	if !allowsRegisteredTenantCreatorAdminMembership(validTenant, "k8s_role_admin", grant) {
		t.Fatal("expected registered tenant creator-admin membership to be allowed")
	}

	for _, testCase := range []struct {
		name   string
		tenant dynamicTenantRole
		member string
		grant  staticMembershipGrant
	}{
		{name: "unregistered tenant", tenant: dynamicTenantRole{name: validTenant.name, login: true, inherit: true, connectionLimit: -1, validUntil: "infinity"}, member: "k8s_role_admin", grant: grant},
		{name: "wrong name", tenant: dynamicTenantRole{name: "tenant", registered: true, login: true, inherit: true, connectionLimit: -1, validUntil: "infinity"}, member: "k8s_role_admin", grant: grant},
		{name: "wrong role attributes", tenant: dynamicTenantRole{name: validTenant.name, registered: true, login: true, connectionLimit: -1, validUntil: "infinity"}, member: "k8s_role_admin", grant: grant},
		{name: "member is not role admin", tenant: validTenant, member: "cyclops_app", grant: grant},
		{name: "self grantor", tenant: validTenant, member: "k8s_role_admin", grant: staticMembershipGrant{grantor: "k8s_role_admin", admin: true, inherit: false, set: false}},
		{name: "wrong admin option", tenant: validTenant, member: "k8s_role_admin", grant: staticMembershipGrant{grantor: "bootstrap_owner", inherit: false, set: false}},
		{name: "wrong inherit option", tenant: validTenant, member: "k8s_role_admin", grant: staticMembershipGrant{grantor: "bootstrap_owner", admin: true, inherit: true, set: false}},
		{name: "wrong set option", tenant: validTenant, member: "k8s_role_admin", grant: staticMembershipGrant{grantor: "bootstrap_owner", admin: true, inherit: false, set: true}},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			if allowsRegisteredTenantCreatorAdminMembership(testCase.tenant, testCase.member, testCase.grant) {
				t.Fatal("unexpectedly allowed tenant creator-admin membership")
			}
		})
	}
}

func TestStaticCreatorAdminMembershipsAllowAbsentOrOneForeignSuperuserGrant(t *testing.T) {
	foreign := staticMembershipGrant{grantor: "bootstrap_owner", grantorSuperuser: true, admin: true, inherit: false, set: false}
	if !staticCreatorAdminMembershipsAreExact("migration_owner", nil, false, nil) {
		t.Fatal("expected absent creator-admin membership to be allowed")
	}
	ownerDrift := staticMembershipGrant{grantor: "migration_owner", admin: true, inherit: true, set: true}
	if !staticCreatorAdminMembershipsAreExact("migration_owner", []staticMembershipGrant{foreign}, false, nil) {
		t.Fatal("expected one foreign superuser creator row to be allowed")
	}
	if !staticCreatorAdminMembershipsAreExact("migration_owner", []staticMembershipGrant{foreign, ownerDrift}, true, nil) {
		t.Fatal("expected one repairable migration-owner row to be allowed")
	}
	delegatedContractGrant := staticMembershipGrant{grantor: "rdsadmin", grantorSuperuser: true, admin: false, inherit: false, set: true}
	if !staticCreatorAdminMembershipsAreExact("migration_owner", []staticMembershipGrant{foreign, delegatedContractGrant}, true, &delegatedContractGrant) {
		t.Fatal("expected an authoritative delegated contract grant to be ignored")
	}
	if staticCreatorAdminMembershipsAreExact("migration_owner", []staticMembershipGrant{foreign, delegatedContractGrant, {grantor: "other_bootstrap_owner", grantorSuperuser: true, admin: true, inherit: false, set: false}}, true, &delegatedContractGrant) {
		t.Fatal("unexpectedly allowed an unrelated foreign creator grant")
	}

	for _, testCase := range []struct {
		name            string
		grants          []staticMembershipGrant
		allowOwnerGrant bool
	}{
		{name: "non-superuser foreign creator", grants: []staticMembershipGrant{{grantor: "foreign_owner", admin: true, inherit: false, set: false}}},
		{name: "wrong foreign options", grants: []staticMembershipGrant{{grantor: "bootstrap_owner", grantorSuperuser: true, admin: true, inherit: true, set: false}}},
		{name: "second foreign creator", grants: []staticMembershipGrant{foreign, {grantor: "other_bootstrap_owner", grantorSuperuser: true, admin: true, inherit: false, set: false}}},
		{name: "owner row is not declared", grants: []staticMembershipGrant{foreign, ownerDrift}},
		{name: "duplicate owner row", grants: []staticMembershipGrant{foreign, ownerDrift, {grantor: "migration_owner", admin: false, inherit: false, set: true}}, allowOwnerGrant: true},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			if staticCreatorAdminMembershipsAreExact("migration_owner", testCase.grants, testCase.allowOwnerGrant, nil) {
				t.Fatal("unexpectedly allowed static creator-admin memberships")
			}
		})
	}
}

func TestStaticMembershipReconciliationPlanRepairsOnlyAfterFailClosedPreflight(t *testing.T) {
	stateOwner := staticMembershipContract{role: "k8s_state_owner", member: "migration_owner", admin: false, inherit: false, set: true}
	reportingOwner := staticMembershipContract{role: "k8s_reporting_owner", member: "migration_owner", admin: false, inherit: false, set: true}
	healthyGrant := staticMembershipGrant{grantor: "migration_owner", admin: false, inherit: false, set: true}
	driftedStateOwnerGrant := staticMembershipGrant{grantor: "migration_owner", admin: false, inherit: false, set: false}

	for _, testCase := range []struct {
		name        string
		grants      [][]staticMembershipGrant
		wantRepairs []staticMembershipContract
		wantError   string
	}{
		{
			name:        "missing state-owner membership",
			grants:      [][]staticMembershipGrant{nil, {healthyGrant}},
			wantRepairs: []staticMembershipContract{stateOwner},
		},
		{
			name:        "set-drifted state-owner membership",
			grants:      [][]staticMembershipGrant{{driftedStateOwnerGrant}, {healthyGrant}},
			wantRepairs: []staticMembershipContract{stateOwner},
		},
		{
			name: "foreign grant blocks earlier state-owner repair",
			grants: [][]staticMembershipGrant{
				nil,
				{{grantor: "foreign_owner", admin: false, inherit: false, set: true}},
			},
			wantError: "grantor foreign_owner",
		},
		{
			name: "duplicate grant blocks earlier state-owner repair",
			grants: [][]staticMembershipGrant{
				nil,
				{healthyGrant, {grantor: "rdsadmin", grantorSuperuser: true, admin: false, inherit: false, set: true}},
			},
			wantError: "2 non-implicit grants",
		},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			repairs, err := staticMembershipReconciliationPlan("migration_owner", []staticMembershipContract{stateOwner, reportingOwner}, testCase.grants)
			if testCase.wantError != "" {
				if err == nil || !strings.Contains(err.Error(), testCase.wantError) {
					t.Fatalf("staticMembershipReconciliationPlan() error = %v, want containing %q", err, testCase.wantError)
				}
				if repairs != nil {
					t.Fatalf("staticMembershipReconciliationPlan() repairs = %#v, want nil before any mutation", repairs)
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			if !reflect.DeepEqual(repairs, testCase.wantRepairs) {
				t.Fatalf("staticMembershipReconciliationPlan() repairs = %#v, want %#v", repairs, testCase.wantRepairs)
			}
		})
	}
}

func TestStaticCreatorAdminMembershipsAllowDelegatedContractGrantAlongsideImplicitCreatorGrant(t *testing.T) {
	implicitCreatorGrant := staticMembershipGrant{grantor: "bootstrap_owner", grantorSuperuser: true, admin: true, inherit: false, set: false}
	delegatedContractGrant := staticMembershipGrant{grantor: "rdsadmin", grantorSuperuser: true, admin: false, inherit: false, set: true}

	if !staticCreatorAdminMembershipsAreExact("migration_owner", []staticMembershipGrant{implicitCreatorGrant, delegatedContractGrant}, true, &delegatedContractGrant) {
		t.Fatal("expected exact delegated contract and implicit creator grants to coexist")
	}
	foreignCreatorGrant := staticMembershipGrant{grantor: "foreign_creator", grantorSuperuser: true, admin: true, inherit: false, set: false}
	if staticCreatorAdminMembershipsAreExact("migration_owner", []staticMembershipGrant{implicitCreatorGrant, delegatedContractGrant, foreignCreatorGrant}, true, &delegatedContractGrant) {
		t.Fatal("unexpectedly allowed unrelated foreign creator grant")
	}
}
func TestCreatorAdminMembershipsRequireOneForeignSuperuserGrant(t *testing.T) {
	valid := staticMembershipGrant{grantor: "bootstrap_owner", grantorSuperuser: true, admin: true, inherit: false, set: false}
	if !creatorAdminMembershipsAreExact("migration_owner", []staticMembershipGrant{valid}) {
		t.Fatal("expected one creator-admin membership from a superuser to be allowed")
	}

	for _, testCase := range []struct {
		name   string
		grants []staticMembershipGrant
	}{
		{name: "missing", grants: nil},
		{name: "non-superuser", grants: []staticMembershipGrant{{grantor: "foreign_owner", admin: true, inherit: false, set: false}}},
		{name: "self grantor", grants: []staticMembershipGrant{{grantor: "migration_owner", grantorSuperuser: true, admin: true, inherit: false, set: false}}},
		{name: "wrong options", grants: []staticMembershipGrant{{grantor: "bootstrap_owner", grantorSuperuser: true, admin: true, inherit: true, set: false}}},
		{name: "duplicate", grants: []staticMembershipGrant{valid, {grantor: "other_bootstrap_owner", grantorSuperuser: true, admin: true, inherit: false, set: false}}},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			if creatorAdminMembershipsAreExact("migration_owner", testCase.grants) {
				t.Fatal("unexpectedly allowed creator-admin memberships")
			}
		})
	}
}

func TestRegisteredTenantMembershipsAreExactInBothDirections(t *testing.T) {
	tenant := dynamicTenantRole{
		name:            "k8s_tenant_0123456789abcdef0123456789abcdef",
		registered:      true,
		login:           true,
		inherit:         true,
		connectionLimit: -1,
		validUntil:      "infinity",
	}
	creator := staticMembershipGrant{grantor: "bootstrap_owner", grantorSuperuser: true, admin: true, inherit: false, set: false}
	controller := staticMembershipGrant{grantor: "k8s_role_admin", admin: false, inherit: true, set: false}
	if !registeredTenantMembershipsAreExact(tenant, []staticMembershipGrant{creator}, []staticMembershipGrant{controller}) {
		t.Fatal("expected healthy registered tenant memberships to be allowed")
	}

	for _, testCase := range []struct {
		name    string
		inbound []staticMembershipGrant
		parents []staticMembershipGrant
	}{
		{name: "arbitrary member", inbound: []staticMembershipGrant{creator, {grantor: "bootstrap_owner", grantorSuperuser: true, admin: true, inherit: false, set: false}}, parents: []staticMembershipGrant{controller}},
		{name: "non-superuser creator", inbound: []staticMembershipGrant{{grantor: "foreign_owner", admin: true, inherit: false, set: false}}, parents: []staticMembershipGrant{controller}},
		{name: "extra creator", inbound: []staticMembershipGrant{creator, {grantor: "other_bootstrap_owner", grantorSuperuser: true, admin: true, inherit: false, set: false}}, parents: []staticMembershipGrant{controller}},
		{name: "extra parent", inbound: []staticMembershipGrant{creator}, parents: []staticMembershipGrant{controller, {grantor: "bootstrap_owner", grantorSuperuser: true, admin: false, inherit: true, set: false}}},
		{name: "wrong controller", inbound: []staticMembershipGrant{creator}, parents: []staticMembershipGrant{{grantor: "k8s_role_admin", admin: true, inherit: true, set: false}}},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			if registeredTenantMembershipsAreExact(tenant, testCase.inbound, testCase.parents) {
				t.Fatal("unexpectedly allowed registered tenant memberships")
			}
		})
	}
}

func TestAllowsImplicitCreatorAdminMembership(t *testing.T) {
	contract := staticMembershipContract{role: "k8s_state_owner", member: "migration_owner"}
	if !allowsImplicitCreatorAdminMembership(contract.role, contract.member, "migration_owner", staticMembershipGrant{
		grantor: "postgres", grantorSuperuser: true, admin: true, inherit: false, set: false,
	}) {
		t.Fatal("expected the PostgreSQL creator-admin membership to be allowed")
	}

	for _, testCase := range []struct {
		name   string
		role   string
		member string
		grant  staticMembershipGrant
	}{
		{name: "non-static parent", role: "unrelated_role", member: "migration_owner", grant: staticMembershipGrant{grantor: "postgres", admin: true}},
		{name: "unrelated member", role: "k8s_state_owner", member: "other", grant: staticMembershipGrant{grantor: "postgres", admin: true}},
		{name: "non-admin", role: contract.role, member: contract.member, grant: staticMembershipGrant{grantor: "postgres", inherit: false, set: false}},
		{name: "inherits", role: contract.role, member: contract.member, grant: staticMembershipGrant{grantor: "postgres", admin: true, inherit: true, set: false}},
		{name: "sets", role: contract.role, member: contract.member, grant: staticMembershipGrant{grantor: "postgres", admin: true, inherit: false, set: true}},
		{name: "self grant", role: contract.role, member: contract.member, grant: staticMembershipGrant{grantor: "migration_owner", admin: true, inherit: false, set: false}},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			if allowsImplicitCreatorAdminMembership(testCase.role, testCase.member, "migration_owner", testCase.grant) {
				t.Fatal("unexpectedly allowed foreign membership")
			}
		})
	}
}

func TestAuthoritativeStaticMembershipGrant(t *testing.T) {
	declaredContract := staticMembershipContract{
		role: "k8s_query_tenant", member: "k8s_role_admin",
		admin: true, inherit: false, set: false,
	}
	creatorContract := staticMembershipContract{
		role: "k8s_state_owner", member: "migration_owner",
		admin: false, inherit: false, set: true,
	}
	implicitCreatorGrant := staticMembershipGrant{
		grantor: "bootstrap_owner", grantorSuperuser: true,
		admin: true, inherit: false, set: false,
	}

	for _, testCase := range []struct {
		name              string
		contract          staticMembershipContract
		grants            []staticMembershipGrant
		wantGrantor       string
		wantError         bool
		wantErrorIs       error
		wantReturnedGrant bool
	}{
		{
			name:        "migration owner grant",
			contract:    declaredContract,
			grants:      []staticMembershipGrant{{grantor: "migration_owner", admin: true, inherit: false, set: false}},
			wantGrantor: "migration_owner",
		},
		{
			name:        "delegated superuser exact grant",
			contract:    declaredContract,
			grants:      []staticMembershipGrant{{grantor: "rdsadmin", grantorSuperuser: true, admin: true, inherit: false, set: false}},
			wantGrantor: "rdsadmin",
		},
		{
			name:        "implicit creator grant is ignored beside owner grant",
			contract:    creatorContract,
			grants:      []staticMembershipGrant{implicitCreatorGrant, {grantor: "migration_owner", admin: false, inherit: false, set: true}},
			wantGrantor: "migration_owner",
		},
		{
			name:        "missing grant",
			contract:    declaredContract,
			wantError:   true,
			wantErrorIs: errStaticMembershipGrantMissing,
		},
		{
			name:        "implicit creator grant does not satisfy declared contract",
			contract:    creatorContract,
			grants:      []staticMembershipGrant{implicitCreatorGrant},
			wantError:   true,
			wantErrorIs: errStaticMembershipGrantMissing,
		},
		{
			name:      "non-superuser grantor",
			contract:  declaredContract,
			grants:    []staticMembershipGrant{{grantor: "foreign_owner", admin: true, inherit: false, set: false}},
			wantError: true,
		},
		{
			name:              "owner admin option drift",
			contract:          declaredContract,
			grants:            []staticMembershipGrant{{grantor: "migration_owner", admin: false, inherit: false, set: false}},
			wantError:         true,
			wantErrorIs:       errStaticMembershipGrantOwnerOptions,
			wantReturnedGrant: true,
		},
		{
			name:              "owner inherit option drift",
			contract:          declaredContract,
			grants:            []staticMembershipGrant{{grantor: "migration_owner", admin: true, inherit: true, set: false}},
			wantError:         true,
			wantErrorIs:       errStaticMembershipGrantOwnerOptions,
			wantReturnedGrant: true,
		},
		{
			name:              "owner set option drift",
			contract:          declaredContract,
			grants:            []staticMembershipGrant{{grantor: "migration_owner", admin: true, inherit: false, set: true}},
			wantError:         true,
			wantErrorIs:       errStaticMembershipGrantOwnerOptions,
			wantReturnedGrant: true,
		},
		{
			name:      "delegated superuser option drift",
			contract:  declaredContract,
			grants:    []staticMembershipGrant{{grantor: "rdsadmin", grantorSuperuser: true, admin: true, inherit: true, set: false}},
			wantError: true,
		},
		{
			name:     "owner and superuser duplicates",
			contract: declaredContract,
			grants: []staticMembershipGrant{
				{grantor: "migration_owner", admin: true, inherit: false, set: false},
				{grantor: "rdsadmin", grantorSuperuser: true, admin: true, inherit: false, set: false},
			},
			wantError: true,
		},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			grant, err := authoritativeStaticMembershipGrant(testCase.contract, "migration_owner", testCase.grants)
			if testCase.wantError {
				if err == nil {
					t.Fatal("expected an error")
				}
				if testCase.wantErrorIs != nil && !errors.Is(err, testCase.wantErrorIs) {
					t.Fatalf("error = %v, want errors.Is(_, %v)", err, testCase.wantErrorIs)
				}
				if (grant != nil) != testCase.wantReturnedGrant {
					t.Fatalf("returned grant = %#v, want present %t", grant, testCase.wantReturnedGrant)
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			if grant.grantor != testCase.wantGrantor {
				t.Fatalf("grantor = %q, want %q", grant.grantor, testCase.wantGrantor)
			}
		})
	}
}

func TestEmbeddedMigrationsAreOrderedAndImmutable(t *testing.T) {
	files, err := embeddedMigrations()
	if err != nil {
		t.Fatal(err)
	}
	if len(files) != 11 {
		t.Fatalf("expected exactly eleven migrations, got %d", len(files))
	}
	manifest := make([]struct {
		Version int64
		Name    string
	}, 0, len(files))
	for _, file := range files {
		manifest = append(manifest, struct {
			Version int64
			Name    string
		}{file.Version, file.Name})
	}
	if !reflect.DeepEqual(manifest, []struct {
		Version int64
		Name    string
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
	}) {
		t.Fatalf("migration manifest = %#v", manifest)
	}
	initial := files[0]
	if initial.Version != 1 || initial.Name != "000001_initial_schema.sql" {
		t.Fatalf("expected version 1 initial schema migration, got version=%d name=%q", initial.Version, initial.Name)
	}
	digest := sha256.Sum256([]byte(initial.SQL))
	if initial.SHA256 != hex.EncodeToString(digest[:]) {
		t.Fatal("embedded initial schema migration digest does not match its contents")
	}
	for _, expected := range []string{
		"CREATE TABLE k8s_state.resource_state",
		"CREATE TABLE k8s_state.watch_checkpoint",
		"CREATE TABLE k8s_state.resource_event_outbox",
		"CREATE TABLE k8s_state.resource_schema",
		"CREATE TABLE k8s_state.query_tenant_role",
		"ALTER TABLE k8s_state.resource_state FORCE ROW LEVEL SECURITY",
		"CREATE POLICY writer_current_state",
		"CREATE POLICY tenant_current_state",
		"resource = 'namespaces'",
		"CREATE VIEW k8s_api.current_resources",
		"GRANT SELECT ON k8s_api.current_resources TO k8s_query_tenant, k8s_query_admin",
		"credential_fingerprint text NOT NULL",
		"CREATE FUNCTION k8s_state.register_tenant_role(p_role_name name, p_capsule_tenant text, p_credential_fingerprint text)",
		"CREATE FUNCTION k8s_state.unregister_tenant_role(p_role_name name)",
		"GRANT EXECUTE ON FUNCTION k8s_state.register_tenant_role(name, text, text) TO k8s_role_admin",
		"GRANT EXECUTE ON FUNCTION k8s_state.unregister_tenant_role(name) TO k8s_role_admin",
	} {
		if !strings.Contains(initial.SQL, expected) {
			t.Errorf("initial schema is missing direct tenant role contract %q", expected)
		}
	}
	if strings.Contains(initial.SQL, "k8s_query_broker") {
		t.Fatal("initial schema must not create or grant a shared query broker role")
	}

	usage := files[1]
	if usage.Version != 2 || usage.Name != "000002_usage_sandbox_events.sql" {
		t.Fatalf("expected version 2 usage migration, got version=%d name=%q", usage.Version, usage.Name)
	}
	for _, expected := range []string{
		"CREATE ROLE cyclops_usage_reader LOGIN NOINHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS",
		"CREATE INDEX resource_event_outbox_usage_lookup_idx",
		"GRANT SELECT ON k8s_state.resource_event_outbox TO k8s_reporting_owner",
		"CREATE FUNCTION k8s_reporting.usage_sandbox_events",
		"SECURITY DEFINER",
		"SET search_path = k8s_state, pg_catalog",
		"REVOKE ALL ON FUNCTION k8s_reporting.usage_sandbox_events(text, timestamptz, timestamptz) FROM PUBLIC",
		"GRANT USAGE ON SCHEMA k8s_reporting TO cyclops_usage_reader",
		"GRANT EXECUTE ON FUNCTION k8s_reporting.usage_sandbox_events(text, timestamptz, timestamptz) TO cyclops_usage_reader",
	} {
		if !strings.Contains(usage.SQL, expected) {
			t.Errorf("usage migration is missing contract %q", expected)
		}
	}
	if strings.Contains(usage.SQL, "GRANT SELECT ON k8s_state.resource_event_outbox TO cyclops_usage_reader") {
		t.Fatal("usage reader must not receive direct outbox table access")
	}

	claimedSandboxPool := files[2]
	if claimedSandboxPool.Version != 3 || claimedSandboxPool.Name != "000003_usage_claimed_sandbox_pool.sql" {
		t.Fatalf("expected version 3 claimed sandbox pool migration, got version=%d name=%q", claimedSandboxPool.Version, claimedSandboxPool.Name)
	}
	for _, expected := range []string{
		"CREATE OR REPLACE FUNCTION k8s_reporting.usage_sandbox_events",
		"event.object -> 'metadata' -> 'annotations' ->> 'osgym.cua.ai/origin-warmpool'",
		"REVOKE ALL ON FUNCTION k8s_reporting.usage_sandbox_events(text, timestamptz, timestamptz) FROM PUBLIC",
		"GRANT EXECUTE ON FUNCTION k8s_reporting.usage_sandbox_events(text, timestamptz, timestamptz) TO cyclops_usage_reader",
	} {
		if !strings.Contains(claimedSandboxPool.SQL, expected) {
			t.Errorf("claimed sandbox pool migration is missing contract %q", expected)
		}
	}

	legacyFilter := files[3]
	if legacyFilter.Version != 4 || legacyFilter.Name != "000004_filter_invalid_usage_sandbox_events.sql" {
		t.Fatalf("expected version 4 usage filter migration, got version=%d name=%q", legacyFilter.Version, legacyFilter.Name)
	}
	for _, expected := range []string{
		"CREATE OR REPLACE FUNCTION k8s_reporting.usage_sandbox_events",
		"event.object -> 'metadata' -> 'annotations' ->> 'osgym.cua.ai/origin-warmpool'",
		"event.object -> 'spec' -> 'vmTemplate' ->> 'runtime' <> ''",
		"event.object -> 'status' ->> 'vmName' <> ''",
	} {
		if !strings.Contains(legacyFilter.SQL, expected) {
			t.Errorf("usage filter migration is missing contract %q", expected)
		}
	}

	meter := files[4]
	if meter.Version != 5 || meter.Name != "000005_hourly_reservation_meter.sql" {
		t.Fatalf("expected version 5 reservation meter migration, got version=%d name=%q", meter.Version, meter.Name)
	}
	for _, expected := range []string{
		"CREATE ROLE billing_meter_owner NOLOGIN",
		"CREATE ROLE cyclops_meter_writer LOGIN",
		"CREATE TABLE billing_meter.reservation_hour_fact",
		"CREATE VIEW billing_meter.reservation_hour_current",
		"BEFORE UPDATE OR DELETE OR TRUNCATE ON billing_meter.reservation_hour_fact",
		"CREATE FUNCTION k8s_api.sandbox_meter_tenant",
		"CREATE FUNCTION k8s_reporting.reservation_hour_facts",
		"GRANT SELECT, INSERT ON TABLE billing_meter.reservation_hour_collection, billing_meter.reservation_hour_fact TO cyclops_meter_writer",
		"GRANT EXECUTE ON FUNCTION k8s_reporting.reservation_hour_facts(text, timestamptz, timestamptz) TO cyclops_usage_reader",
	} {
		if !strings.Contains(meter.SQL, expected) {
			t.Errorf("reservation meter migration is missing contract %q", expected)
		}
	}

	chatConversations := files[5]
	if chatConversations.Version != 6 || chatConversations.Name != "000006_chat_conversations.sql" {
		t.Fatalf("expected version 6 chat migration, got version=%d name=%q", chatConversations.Version, chatConversations.Name)
	}
	for _, expected := range []string{
		"CREATE TABLE public.chat_conversations",
		"messages jsonb NOT NULL DEFAULT '[]'::jsonb",
		"CREATE INDEX chat_conversations_owner_active_idx",
		"CREATE INDEX chat_conversations_owner_archived_idx",
		"GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.chat_conversations TO cyclops_app",
	} {
		if !strings.Contains(chatConversations.SQL, expected) {
			t.Errorf("chat conversation migration is missing contract %q", expected)
		}
	}

	metabaseUsage := files[6]
	if metabaseUsage.Version != 7 || metabaseUsage.Name != "000007_metabase_hourly_reservation_usage.sql" {
		t.Fatalf("expected version 7 Metabase usage migration, got version=%d name=%q", metabaseUsage.Version, metabaseUsage.Name)
	}
	for _, expected := range []string{
		"CREATE VIEW k8s_reporting.hourly_reservation_usage",
		"FROM billing_meter.reservation_hour_collection_current AS collection",
		"LEFT JOIN billing_meter.reservation_hour_current AS fact",
		"GRANT SELECT ON k8s_reporting.hourly_reservation_usage TO k8s_metabase",
	} {
		if !strings.Contains(metabaseUsage.SQL, expected) {
			t.Errorf("Metabase usage migration is missing contract %q", expected)
		}
	}

	filteredMetabaseUsage := files[7]
	if filteredMetabaseUsage.Version != 8 || filteredMetabaseUsage.Name != "000008_metabase_hourly_reservation_usage_excluding_tenants.sql" {
		t.Fatalf("expected version 8 filtered Metabase usage migration, got version=%d name=%q", filteredMetabaseUsage.Version, filteredMetabaseUsage.Name)
	}
	for _, expected := range []string{
		"CREATE VIEW k8s_reporting.hourly_reservation_usage_excluding_tenants",
		"fact.capsule_tenant NOT IN",
		"user-f039fe89-9b5f-43dc-8ccd-d100ae732246",
		"user-30a53246-881d-4f1a-8005-979f2a07933e",
		"GRANT SELECT ON k8s_reporting.hourly_reservation_usage_excluding_tenants TO k8s_metabase",
	} {
		if !strings.Contains(filteredMetabaseUsage.SQL, expected) {
			t.Errorf("filtered Metabase usage migration is missing contract %q", expected)
		}
	}
	extendedFilteredMetabaseUsage := files[8]
	if extendedFilteredMetabaseUsage.Version != 9 || extendedFilteredMetabaseUsage.Name != "000009_extend_metabase_revenue_tenant_exclusions.sql" {
		t.Fatalf("expected version 9 extended filtered Metabase usage migration, got version=%d name=%q", extendedFilteredMetabaseUsage.Version, extendedFilteredMetabaseUsage.Name)
	}
	for _, expected := range []string{
		"CREATE OR REPLACE VIEW k8s_reporting.hourly_reservation_usage_excluding_tenants",
		"GRANT SELECT ON k8s_reporting.hourly_reservation_usage_excluding_tenants TO k8s_metabase",
	} {
		if !strings.Contains(extendedFilteredMetabaseUsage.SQL, expected) {
			t.Errorf("extended filtered Metabase usage migration is missing contract %q", expected)
		}
	}
	predicate := regexp.MustCompile(`(?s)\bfact\.capsule_tenant\s+NOT\s+IN\s*\((.*?)\)`).FindStringSubmatch(extendedFilteredMetabaseUsage.SQL)
	if len(predicate) != 2 {
		t.Fatal("version 9 migration does not contain a fact.capsule_tenant NOT IN predicate")
	}
	quotedLabel := regexp.MustCompile(`'([^']*)'`)
	labels := make([]string, 0)
	for _, match := range quotedLabel.FindAllStringSubmatch(predicate[1], -1) {
		labels = append(labels, match[1])
	}
	wantLabels := []string{
		"user-f039fe89-9b5f-43dc-8ccd-d100ae732246",
		"user-30a53246-881d-4f1a-8005-979f2a07933e",
		"user-0ea07f31-b7bd-4e99-b29a-2376f6fde1be",
		"user-a89b2628-9656-4ef0-bf01-e925b120ed1d",
	}
	if !reflect.DeepEqual(labels, wantLabels) {
		t.Fatalf("version 9 filtered tenant labels = %#v, want %#v", labels, wantLabels)
	}
	if remaining := strings.Trim(quotedLabel.ReplaceAllString(predicate[1], ""), " \t\r\n,"); remaining != "" {
		t.Fatalf("version 9 filtered tenant predicate contains non-label content %q", remaining)
	}
	billingMeterAccess := files[9]
	if billingMeterAccess.Version != 10 || billingMeterAccess.Name != "000010_grant_metabase_billing_meter_access.sql" {
		t.Fatalf("expected version 10 Metabase billing meter access migration, got version=%d name=%q", billingMeterAccess.Version, billingMeterAccess.Name)
	}
	for _, expected := range []string{
		"SET LOCAL ROLE billing_meter_owner",
		"GRANT USAGE ON SCHEMA billing_meter TO k8s_metabase",
		"GRANT SELECT ON ALL TABLES IN SCHEMA billing_meter TO k8s_metabase",
		"ALTER DEFAULT PRIVILEGES IN SCHEMA billing_meter GRANT SELECT ON TABLES TO k8s_metabase",
	} {
		if !strings.Contains(billingMeterAccess.SQL, expected) {
			t.Errorf("Metabase billing meter access migration is missing contract %q", expected)
		}
	}

	signedServiceURLs := files[10]
	for _, fragment := range []string{
		"CREATE TABLE public.signed_service_urls",
		"signed_service_urls_claim_created_idx",
		"CHECK (expires_at >= created_at + interval '1 minute')",
		"CHECK (expires_at <= created_at + interval '24 hours')",
		"GRANT SELECT, INSERT, UPDATE ON TABLE public.signed_service_urls TO cyclops_app",
	} {
		if !strings.Contains(signedServiceURLs.SQL, fragment) {
			t.Fatalf("migration 11 missing %q", fragment)
		}
	}

}

func TestReservationMeterMigrationRetainsRecordedChecksum(t *testing.T) {
	files, err := embeddedMigrations()
	if err != nil {
		t.Fatal(err)
	}
	meter := files[4]
	if meter.SHA256 != hourlyReservationMeterOriginalSHA256 {
		t.Fatalf("reservation meter checksum = %s, want recorded %s", meter.SHA256, hourlyReservationMeterOriginalSHA256)
	}
}

func TestExecutableMigrationSQLRepairsLegacyPrivilegeOrder(t *testing.T) {
	file := migrationFile{
		Version: 5,
		Name:    "000005_hourly_reservation_meter.sql",
		SHA256:  hourlyReservationMeterOriginalSHA256,
		SQL:     hourlyReservationMeterPrivilegeSequence,
	}

	executable, err := prepareMigrationExecution(file)
	if err != nil {
		t.Fatal(err)
	}
	if executable.SQL != hourlyReservationMeterCompatiblePrivilegeSequence {
		t.Fatalf("executable migration SQL = %q", executable.SQL)
	}
	if file.SQL != hourlyReservationMeterPrivilegeSequence {
		t.Fatal("compatibility repair must not mutate immutable migration bytes")
	}
}

func TestExecutableMigrationSQLRejectsAmbiguousLegacyMigration(t *testing.T) {
	_, err := prepareMigrationExecution(migrationFile{
		Name:   "000005_hourly_reservation_meter.sql",
		SHA256: hourlyReservationMeterOriginalSHA256,
		SQL:    hourlyReservationMeterPrivilegeSequence + "\n" + hourlyReservationMeterPrivilegeSequence,
	})
	if err == nil || !strings.Contains(err.Error(), "missing or ambiguous") {
		t.Fatalf("expected ambiguous legacy sequence error, got %v", err)
	}
}

func TestAppliedMigrationLedgerUsesApplicationOrder(t *testing.T) {
	if !strings.Contains(createAppliedMigrationsTableStatement, "application_order bigint generated always as identity unique") {
		t.Fatalf("ledger schema must define a unique identity application order: %s", createAppliedMigrationsTableStatement)
	}
	if !strings.HasSuffix(selectAppliedMigrationsStatement, "order by application_order") {
		t.Fatalf("ledger query must preserve application order: %s", selectAppliedMigrationsStatement)
	}
	if strings.Contains(insertAppliedMigrationStatement, "application_order") {
		t.Fatalf("ledger insert must allow the identity application order to be generated: %s", insertAppliedMigrationStatement)
	}
}

func TestCheckAppliedMigrationRejectsChangedChecksum(t *testing.T) {
	err := checkAppliedMigration(
		migrationFile{Version: 1, Name: "000001_initial_schema.sql", SHA256: "current"},
		appliedMigration{Version: 1, Name: "000001_initial_schema.sql", SHA256: "recorded"},
	)
	if err == nil {
		t.Fatal("expected checksum mismatch")
	}
}

func TestValidateAppliedMigrationsRejectsFilenameMismatch(t *testing.T) {
	err := validateAppliedMigrations(
		[]migrationFile{{Version: 1, Name: "000001_initial_schema.sql", SHA256: "checksum"}},
		[]appliedMigration{{Version: 1, Name: "000001_renamed.sql", SHA256: "checksum"}},
	)
	if err == nil || !strings.Contains(err.Error(), "filename changed") {
		t.Fatalf("expected filename mismatch, got %v", err)
	}
}

func TestValidateAppliedMigrationsRejectsOrphanedLedgerRow(t *testing.T) {
	err := validateAppliedMigrations(
		[]migrationFile{{Version: 1, Name: "000001_initial_schema.sql", SHA256: "checksum"}},
		[]appliedMigration{{Version: 2, Name: "000002_deleted.sql", SHA256: "checksum"}},
	)
	if err == nil || !strings.Contains(err.Error(), "no embedded migration") {
		t.Fatalf("expected orphaned ledger row, got %v", err)
	}
}

func TestValidateAppliedMigrationsRejectsNonContiguousLedger(t *testing.T) {
	files := []migrationFile{
		{Version: 1, Name: "000001_initial_schema.sql", SHA256: "first"},
		{Version: 2, Name: "000002_add_roles.sql", SHA256: "second"},
		{Version: 3, Name: "000003_add_widgets.sql", SHA256: "third"},
	}
	tests := []struct {
		name    string
		applied []appliedMigration
		message string
	}{
		{
			name: "gap",
			applied: []appliedMigration{
				{Version: 1, Name: "000001_initial_schema.sql", SHA256: "first"},
				{Version: 3, Name: "000003_add_widgets.sql", SHA256: "third"},
			},
			message: "migration ledger version gap: expected 000002, got 000003",
		},
		{
			name: "out of order",
			applied: []appliedMigration{
				{Version: 1, Name: "000001_initial_schema.sql", SHA256: "first"},
				{Version: 3, Name: "000003_add_widgets.sql", SHA256: "third"},
				{Version: 2, Name: "000002_add_roles.sql", SHA256: "second"},
			},
			message: "migration ledger rows are out of order: version 000002 follows 000003",
		},
		{
			name: "duplicate",
			applied: []appliedMigration{
				{Version: 1, Name: "000001_initial_schema.sql", SHA256: "first"},
				{Version: 1, Name: "000001_initial_schema.sql", SHA256: "first"},
			},
			message: "migration ledger contains duplicate version 000001",
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			err := validateAppliedMigrations(files, test.applied)
			if err == nil || err.Error() != test.message {
				t.Fatalf("expected %q, got %v", test.message, err)
			}
		})
	}
}

func TestValidateAppliedMigrationsAcceptsMatchingLedger(t *testing.T) {
	files := []migrationFile{
		{Version: 1, Name: "000001_initial_schema.sql", SHA256: "first"},
		{Version: 2, Name: "000002_add_roles.sql", SHA256: "second"},
	}
	applied := []appliedMigration{
		{Version: 1, Name: "000001_initial_schema.sql", SHA256: "first"},
		{Version: 2, Name: "000002_add_roles.sql", SHA256: "second"},
	}
	if err := validateAppliedMigrations(files, applied); err != nil {
		t.Fatal(err)
	}
	if current := migrationCurrentVersion(applied); current != 2 {
		t.Fatalf("expected current version 2, got %d", current)
	}
}

func TestCredentialURLsExcludeDynamicTenantRoles(t *testing.T) {
	credentials, err := parseCredentialURLs(CredentialURLs{
		Application: "postgres://cyclops_app:pw@db/cyclops",
		Writer:      "postgres://k8s_state_writer:pw@db/cyclops",
		Exporter:    "postgres://k8s_state_exporter:pw@db/cyclops",
		RoleAdmin:   "postgres://k8s_role_admin:pw@db/cyclops",
		Metabase:    "postgres://k8s_metabase:pw@db/cyclops",
		Usage:       "postgres://cyclops_usage_reader:pw@db/cyclops",
		Meter:       "postgres://cyclops_meter_writer:pw@db/cyclops",
	})
	if err != nil {
		t.Fatalf("parse fixed runtime credentials: %v", err)
	}
	if len(credentials) != 7 {
		t.Fatalf("credential count = %d, want 7", len(credentials))
	}
	for _, credential := range credentials {
		if credential.Role == "k8s_query_broker" || strings.HasPrefix(credential.Role, "k8s_tenant_") {
			t.Fatalf("migrator must not reconcile dynamic tenant query credentials: %q", credential.Role)
		}
	}
}

func TestStaticRoleContractsAreAllowlistedAndFixed(t *testing.T) {
	contracts := staticRoleContracts()
	want := map[string]struct {
		login, inherit, createRole, createDB bool
		connectionLimit                      int
		validUntil                           staticRoleValidUntil
	}{
		"cyclops_app":          {true, false, false, false, -1, staticRoleValidUntilInfinity},
		"k8s_state_owner":      {false, true, false, false, -1, staticRoleValidUntilInfinity},
		"k8s_state_writer":     {true, false, false, false, -1, staticRoleValidUntilInfinity},
		"k8s_state_exporter":   {true, false, false, false, -1, staticRoleValidUntilInfinity},
		"k8s_query_tenant":     {false, true, false, false, -1, staticRoleValidUntilInfinity},
		"k8s_query_admin":      {false, true, false, false, -1, staticRoleValidUntilInfinity},
		"k8s_role_admin":       {true, false, true, false, -1, staticRoleValidUntilInfinity},
		"k8s_reporting_owner":  {false, true, false, false, -1, staticRoleValidUntilInfinity},
		"billing_meter_owner":  {false, true, false, false, -1, staticRoleValidUntilInfinity},
		"k8s_metabase":         {true, false, false, false, -1, staticRoleValidUntilInfinity},
		"cyclops_usage_reader": {true, false, false, false, -1, staticRoleValidUntilInfinity},
		"cyclops_meter_writer": {true, false, false, false, -1, staticRoleValidUntilInfinity},
	}
	if len(contracts) != len(want) {
		t.Fatalf("static role contract count = %d, want %d", len(contracts), len(want))
	}
	for _, contract := range contracts {
		expected, ok := want[contract.role]
		if !ok {
			t.Fatalf("static role contract includes non-allowlisted role %q", contract.role)
		}
		if contract.login != expected.login || contract.inherit != expected.inherit || contract.createRole != expected.createRole || contract.createDB != expected.createDB || contract.connectionLimit != expected.connectionLimit || contract.validUntil != expected.validUntil {
			t.Errorf("role %s contract = login:%t inherit:%t createrole:%t createdb:%t connection_limit:%d valid_until:%d, want login:%t inherit:%t createrole:%t createdb:%t connection_limit:%d valid_until:%d", contract.role, contract.login, contract.inherit, contract.createRole, contract.createDB, contract.connectionLimit, contract.validUntil, expected.login, expected.inherit, expected.createRole, expected.createDB, expected.connectionLimit, expected.validUntil)
		}
		delete(want, contract.role)
	}
	if len(want) != 0 {
		t.Fatalf("static role contract omitted roles: %v", want)
	}
}

func TestStaticMembershipContractsUsePG16Options(t *testing.T) {
	contracts := staticMembershipContracts("migration_owner")
	want := []staticMembershipContract{
		{role: "k8s_state_owner", member: "migration_owner", admin: false, inherit: false, set: true},
		{role: "k8s_reporting_owner", member: "migration_owner", admin: false, inherit: false, set: true},
		{role: "billing_meter_owner", member: "migration_owner", admin: false, inherit: false, set: true},
		{role: "k8s_query_tenant", member: "k8s_role_admin", admin: true, inherit: false, set: false},
		{role: "k8s_query_admin", member: "k8s_reporting_owner", admin: false, inherit: true, set: false},
	}
	if !reflect.DeepEqual(contracts, want) {
		t.Fatalf("static membership contracts = %#v, want %#v", contracts, want)
	}
}

func TestStaticRolesAreAllMembershipValidationTargets(t *testing.T) {
	for _, contract := range staticRoleContracts() {
		if !isMigrationOwnedStaticRole(contract.role) {
			t.Errorf("static role %q is omitted from membership validation", contract.role)
		}
	}
}

func TestCredentialURLsRequireExpectedRoleNames(t *testing.T) {
	_, err := parseCredentialURLs(CredentialURLs{
		Application: "postgres://wrong:pw@db/cyclops",
		Writer:      "postgres://k8s_state_writer:pw@db/cyclops",
		Exporter:    "postgres://k8s_state_exporter:pw@db/cyclops",
		RoleAdmin:   "postgres://k8s_role_admin:pw@db/cyclops",
		Metabase:    "postgres://k8s_metabase:pw@db/cyclops",
		Usage:       "postgres://cyclops_usage_reader:pw@db/cyclops",
		Meter:       "postgres://cyclops_meter_writer:pw@db/cyclops",
	})
	if err == nil {
		t.Fatal("expected application role-name validation")
	}
	if strings.Contains(err.Error(), "wrong") || strings.Contains(err.Error(), "cyclops_app") {
		t.Fatalf("credential error leaked a username: %v", err)
	}
}

func TestCredentialURLsRejectEmptyPassword(t *testing.T) {
	_, err := parseCredentialURLs(CredentialURLs{
		Application: "postgres://cyclops_app:@db/cyclops",
		Writer:      "postgres://k8s_state_writer:pw@db/cyclops",
		Exporter:    "postgres://k8s_state_exporter:pw@db/cyclops",
		RoleAdmin:   "postgres://k8s_role_admin:pw@db/cyclops",
		Metabase:    "postgres://k8s_metabase:pw@db/cyclops",
		Usage:       "postgres://cyclops_usage_reader:pw@db/cyclops",
		Meter:       "postgres://cyclops_meter_writer:pw@db/cyclops",
	})
	if err == nil || err.Error() != "application credential database URL must include a password" {
		t.Fatalf("expected empty password rejection, got %v", err)
	}
}

func TestCredentialURLParseErrorsPreserveCauseForBoundaryClassification(t *testing.T) {
	_, err := parseCredentialURLs(CredentialURLs{
		Application: "postgres://leaked-user:leaked-password@%zz/cyclops",
		Writer:      "postgres://k8s_state_writer:pw@db/cyclops",
		Exporter:    "postgres://k8s_state_exporter:pw@db/cyclops",
		RoleAdmin:   "postgres://k8s_role_admin:pw@db/cyclops",
		Metabase:    "postgres://k8s_metabase:pw@db/cyclops",
		Usage:       "postgres://cyclops_usage_reader:pw@db/cyclops",
		Meter:       "postgres://cyclops_meter_writer:pw@db/cyclops",
	})
	if !errors.Is(err, ErrInvalidConfiguration) {
		t.Fatalf("error = %v, want ErrInvalidConfiguration", err)
	}
	var parseErr *pgconn.ParseConfigError
	if !errors.As(err, &parseErr) {
		t.Fatalf("error = %v, want preserved *pgconn.ParseConfigError", err)
	}
}

func TestMigrationConfigAndLoggingAreSafe(t *testing.T) {
	const username = "leaked-user"
	const password = "leaked-password"
	const databaseURL = "postgres://" + username + ":" + password + "@db.example.test/cyclops"

	connectionConfig, err := pgx.ParseConfig(databaseURL)
	if err != nil {
		t.Fatal(err)
	}
	target := databaseTarget(connectionConfig)
	if !strings.Contains(target, "db.example.test") || !strings.Contains(target, "cyclops") {
		t.Fatalf("expected database target, got %q", target)
	}
	if strings.Contains(target, username) || strings.Contains(target, password) || strings.Contains(target, databaseURL) {
		t.Fatalf("database target leaked secret data: %q", target)
	}

	_, err = parseMigrationConfig("postgres://" + username + ":" + password + "@%zz/cyclops")
	if !errors.Is(err, ErrInvalidConfiguration) {
		t.Fatalf("error = %v, want ErrInvalidConfiguration", err)
	}
	var parseErr *pgconn.ParseConfigError
	if !errors.As(err, &parseErr) {
		t.Fatalf("error = %v, want preserved *pgconn.ParseConfigError", err)
	}

	var output bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewJSONHandler(&output, nil)))
	t.Cleanup(func() { slog.SetDefault(previous) })
	logMigrationSummary(migrationSummary{
		DatabaseHost: connectionConfig.Host,
		DatabaseName: connectionConfig.Database,
		Started:      time.Now(),
		Result:       "success",
	})
	if strings.Contains(output.String(), username) || strings.Contains(output.String(), password) || strings.Contains(output.String(), databaseURL) {
		t.Fatalf("migration summary leaked secret data: %s", output.String())
	}
}
