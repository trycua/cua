package templates

import (
	"context"
	"encoding/json"
	"testing"
	"time"
)

func TestKeyLayoutIsUserScoped(t *testing.T) {
	// A template key always carries its owner, so one user's name can
	// never resolve to another user's hash.
	if got := tplKey("user-a", "tpl"); got != "pooltpl:user-a:tpl" {
		t.Fatalf("tplKey = %q", got)
	}
	if got := indexKey("user-a"); got != "pooltpl:index:user-a" {
		t.Fatalf("indexKey = %q", got)
	}
	if tplKey("user-a", "tpl") == tplKey("user-b", "tpl") {
		t.Fatal("identical template name across users must not collide")
	}
}

func TestHydrateFallsBackToAddressedKey(t *testing.T) {
	// Missing user/name fields fall back to the addressed components;
	// missing config becomes JSON null, not an empty (invalid) blob.
	got := hydrate("user-a", "tpl", map[string]string{
		"created_at": "2026-05-01T10:00:00Z",
	})
	if got.User != "user-a" || got.Name != "tpl" {
		t.Fatalf("fallback user/name = %q/%q", got.User, got.Name)
	}
	if string(got.Config) != "null" {
		t.Fatalf("empty config = %q, want null", string(got.Config))
	}
	if got.CreatedAt.IsZero() {
		t.Fatal("created_at not parsed")
	}
}

func TestHydratePrefersStoredFields(t *testing.T) {
	got := hydrate("addr-user", "addr-name", map[string]string{
		"user":   "stored-user",
		"name":   "stored-name",
		"config": `{"cpu":4}`,
	})
	if got.User != "stored-user" || got.Name != "stored-name" {
		t.Fatalf("stored user/name = %q/%q", got.User, got.Name)
	}
	if string(got.Config) != `{"cpu":4}` {
		t.Fatalf("config = %q", string(got.Config))
	}
}

func TestSortByCreatedDescNewestFirst(t *testing.T) {
	mk := func(name string, ts string) *Template {
		at, _ := time.Parse(time.RFC3339, ts)
		return &Template{Name: name, CreatedAt: at, Config: json.RawMessage("null")}
	}
	ts := []*Template{
		mk("old", "2026-01-01T00:00:00Z"),
		mk("new", "2026-03-01T00:00:00Z"),
		mk("mid", "2026-02-01T00:00:00Z"),
	}
	sortByCreatedDesc(ts)
	if ts[0].Name != "new" || ts[1].Name != "mid" || ts[2].Name != "old" {
		t.Fatalf("order = %s,%s,%s", ts[0].Name, ts[1].Name, ts[2].Name)
	}
}

func TestNewEmptyURLDisabled(t *testing.T) {
	// No URL → (nil, nil): the caller treats nil as "disabled" and the
	// handlers return 503 rather than silently dropping writes.
	store, err := New(context.Background(), "")
	if err != nil {
		t.Fatalf("New(\"\") err = %v", err)
	}
	if store != nil {
		t.Fatal("New(\"\") store should be nil")
	}
}

func TestNewInvalidURLErrors(t *testing.T) {
	if _, err := New(context.Background(), "not-a-redis-url"); err == nil {
		t.Fatal("New with garbage URL should error")
	}
}
