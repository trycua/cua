package githubtrust

import (
	"context"
	"os"
	"testing"
)

// TestPostgresStoreRoundTrip exercises the real Postgres-backed store against
// a live database. It is skipped unless CYCLOPS_TEST_DATABASE_URL points at a
// throwaway Postgres (e.g. `docker run --rm -e POSTGRES_PASSWORD=pg -p
// 5432:5432 postgres:16`). The test creates the schema on its own via New.
func TestPostgresStoreRoundTrip(t *testing.T) {
	url := os.Getenv("CYCLOPS_TEST_DATABASE_URL")
	if url == "" {
		t.Skip("set CYCLOPS_TEST_DATABASE_URL to run the Postgres store test")
	}
	ctx := context.Background()
	store, err := New(ctx, url)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	if store == nil {
		t.Fatal("New returned a nil store for a non-empty url")
	}

	const owner = "user-roundtrip"
	policy, err := NormalizePolicyInput(PolicyInput{
		Name:              "ci",
		Repository:        "trycua/cloud",
		AllowedNamespaces: []string{"ns-a", "ns-b"},
		Enabled:           true,
	})
	if err != nil {
		t.Fatalf("NormalizePolicyInput: %v", err)
	}
	policy.OwnerSub = owner
	if err := store.Create(ctx, policy); err != nil {
		t.Fatalf("Create: %v", err)
	}
	t.Cleanup(func() { _, _ = store.Delete(ctx, owner, policy.ID) })

	got, err := store.Get(ctx, owner, policy.ID)
	if err != nil || got == nil {
		t.Fatalf("Get: %v (got %v)", err, got)
	}
	if got.Repository != "trycua/cloud" || len(got.AllowedNamespaces) != 2 {
		t.Fatalf("unexpected policy: %#v", got)
	}

	// Tenant isolation: another owner cannot see it.
	if other, err := store.Get(ctx, "someone-else", policy.ID); err != nil || other != nil {
		t.Fatalf("expected nil for other owner, got %v (err %v)", other, err)
	}

	resolved, err := store.ResolveByRepository(ctx, "trycua/cloud")
	if err != nil || len(resolved) == 0 {
		t.Fatalf("ResolveByRepository: %v (n=%d)", err, len(resolved))
	}

	policy.Enabled = false
	if err := store.Update(ctx, policy); err != nil {
		t.Fatalf("Update: %v", err)
	}

	found, err := store.Delete(ctx, owner, policy.ID)
	if err != nil || !found {
		t.Fatalf("Delete: %v (found %v)", err, found)
	}
	if missing, err := store.Get(ctx, owner, policy.ID); err != nil || missing != nil {
		t.Fatalf("expected deleted policy to be gone, got %v (err %v)", missing, err)
	}
}
