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
// flagsData() → input.flags → the prepared OPA query. It guards the
// glue (flag discovery, env→key mapping, result decoding) that the
// pure-policy tests in policy_test.go don't touch.
func TestExportedEvalsEndToEnd(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-sub"]`)

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

func TestFlagsDataLoadsCardRequirementExemptSubs(t *testing.T) {
	t.Setenv("CYCLOPS_CS_CARD_REQUIREMENT_EXEMPT_SUBS", `["exempt-a","exempt-b"]`)
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup dev provider: %v", err)
	}
	resetFlagsCache()

	got := asStrings(flagsData()["card_requirement_exempt_subs"])
	if len(got) != 2 || got[0] != "exempt-a" || got[1] != "exempt-b" {
		t.Fatalf("card_requirement_exempt_subs = %v, want [exempt-a exempt-b]", got)
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

func TestEvalBillingEnabledDefaultsFalseAndReadsBooleanFlag(t *testing.T) {
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup dev provider: %v", err)
	}
	LoadOpa()

	t.Setenv("CYCLOPS_CS_BILLING_ENABLED", "false")
	resetFlagsCache()
	enabled, err := EvalBillingEnabled(context.Background(), &User{ID: "user-1", AZP: "cyclops-cs-spa"})
	if err != nil || enabled {
		t.Fatalf("EvalBillingEnabled(false) = %v, %v; want false, nil", enabled, err)
	}

	t.Setenv("CYCLOPS_CS_BILLING_ENABLED", "true")
	resetFlagsCache()
	enabled, err = EvalBillingEnabled(context.Background(), &User{ID: "user-1", AZP: "cyclops-cs-spa"})
	if err != nil || !enabled {
		t.Fatalf("EvalBillingEnabled(true) = %v, %v; want true, nil", enabled, err)
	}
}
