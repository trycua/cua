package auth

import (
	"context"
	"testing"
	"time"

	"github.com/trycua/cloud/pkg/featureflags"
)

// resetFlagsCache clears the TTL cache so the next flagsData() re-resolves
// from the (test-controlled) provider/env.
func resetFlagsCache() {
	flagsMu.Lock()
	flagsValue = nil
	flagsExp = time.Time{}
	flagsMu.Unlock()
}

// TestExportedEvalsEndToEnd wires the real path: SimpleEnvProvider →
// flagsData() → input.flags → the prepared OPA queries. It guards the
// glue (flag discovery, env→key mapping, set-result decoding) that the
// pure-policy tests in policy_test.go don't touch.
func TestExportedEvalsEndToEnd(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-sub"]`)
	t.Setenv("CYCLOPS_CS_EVENT_REASON_ALLOWLIST", `["PoolCreated","PoolReady"]`)

	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup dev provider: %v", err)
	}
	LoadOpa()
	resetFlagsCache() // force a fresh resolve now that env is set

	ctx := context.Background()

	t.Run("EvalIsAdmin", func(t *testing.T) {
		ok, err := EvalIsAdmin(ctx, &User{ID: "admin-sub", AZP: "cyclops-cs-spa"})
		if err != nil || !ok {
			t.Fatalf("EvalIsAdmin(admin) = %v, %v; want true, nil", ok, err)
		}
		ok, err = EvalIsAdmin(ctx, &User{ID: "someone-else", AZP: "cyclops-cs-spa"})
		if err != nil || ok {
			t.Fatalf("EvalIsAdmin(non-admin) = %v, %v; want false, nil", ok, err)
		}
		ok, err = EvalIsAdmin(ctx, nil)
		if err != nil || ok {
			t.Fatalf("EvalIsAdmin(nil) = %v, %v; want false, nil", ok, err)
		}
	})

	t.Run("EvalVisibleEvents", func(t *testing.T) {
		items := []interface{}{
			map[string]interface{}{"reason": "PoolCreated"},
			map[string]interface{}{"reason": "PoolReady"},
			map[string]interface{}{"reason": "InternalScaleDecision"},
		}

		out, err := EvalVisibleEvents(ctx, &User{ID: "nobody"}, items)
		if err != nil {
			t.Fatalf("EvalVisibleEvents(non-admin): %v", err)
		}
		if len(out) != 2 {
			t.Fatalf("non-admin visible events = %d, want 2 (allowlisted reasons only)", len(out))
		}

		out, err = EvalVisibleEvents(ctx, &User{ID: "admin-sub"}, items)
		if err != nil {
			t.Fatalf("EvalVisibleEvents(admin): %v", err)
		}
		if len(out) != 3 {
			t.Fatalf("admin visible events = %d, want 3 (all)", len(out))
		}
	})
}

// TestFlagsDataCachedAndRefreshes confirms the TTL cache serves a stable
// value within the window and re-resolves lazily (ad-hoc, no background
// timer) once expired.
func TestFlagsDataCachedAndRefreshes(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["a"]`)
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup dev provider: %v", err)
	}
	resetFlagsCache()

	first := flagsData()
	if got := asStrings(first["admin_subs"]); len(got) != 1 || got[0] != "a" {
		t.Fatalf("admin_subs = %v, want [a]", got)
	}

	// Within the TTL the cached value is returned even though env changed.
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["a","b"]`)
	if got := asStrings(flagsData()["admin_subs"]); len(got) != 1 {
		t.Fatalf("expected cached value within TTL, got %v", got)
	}

	// After expiry the next call re-resolves and picks up the change.
	flagsMu.Lock()
	flagsExp = time.Now().Add(-time.Second)
	flagsMu.Unlock()
	if got := asStrings(flagsData()["admin_subs"]); len(got) != 2 {
		t.Fatalf("expected refreshed value after expiry, got %v", got)
	}
}

func asStrings(v any) []string {
	items, ok := v.([]interface{})
	if !ok {
		return nil
	}
	out := make([]string, 0, len(items))
	for _, it := range items {
		if s, ok := it.(string); ok {
			out = append(out, s)
		}
	}
	return out
}
