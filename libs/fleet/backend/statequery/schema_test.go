package statequery

import (
	"encoding/hex"
	"strings"
	"testing"
)

func TestMigrationContainsRequiredObjects(t *testing.T) {
	migration, err := migrationSQL("0001_postgres_state.sql")
	if err != nil {
		t.Fatal(err)
	}

	for _, required := range []string{
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
		"GRANT SELECT ON k8s_state.query_tenant_role TO k8s_role_admin",
	} {
		if !strings.Contains(migration, required) {
			t.Errorf("migration missing %q", required)
		}
	}
	if strings.Contains(migration, "k8s_query_broker") {
		t.Fatal("migration retains the removed query broker")
	}
}

func TestEmbeddedMigrationsAreSortedAndDigested(t *testing.T) {
	files, err := embeddedMigrations()
	if err != nil {
		t.Fatal(err)
	}
	if len(files) != 1 {
		t.Fatalf("len(files) = %d, want 1", len(files))
	}
	if files[0].Name != "0001_postgres_state.sql" {
		t.Fatalf("Name = %q", files[0].Name)
	}
	if len(files[0].Digest) != 64 {
		t.Fatalf("digest length = %d, want 64", len(files[0].Digest))
	}
	if _, err := hex.DecodeString(files[0].Digest); err != nil {
		t.Fatalf("digest is not hexadecimal: %v", err)
	}
}

func TestCheckAppliedDigestRejectsChangedMigration(t *testing.T) {
	if err := checkAppliedDigest("0001.sql", "old", "new"); err == nil {
		t.Fatal("expected changed migration digest to fail")
	}
	if err := checkAppliedDigest("0001.sql", "same", "same"); err != nil {
		t.Fatalf("unchanged digest failed: %v", err)
	}
}
