package auth

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"reflect"
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
	flagsGeneration = 0
	flagsMu.Unlock()
	flagsSF.Forget("flags")
}

func unsetEnv(t *testing.T, key string) {
	t.Helper()
	value, present := os.LookupEnv(key)
	if err := os.Unsetenv(key); err != nil {
		t.Fatalf("unset %s: %v", key, err)
	}
	t.Cleanup(func() {
		if present {
			_ = os.Setenv(key, value)
			return
		}
		_ = os.Unsetenv(key)
	})
}

func TestEvalChatEnabledResolvesGlobalAccessFlag(t *testing.T) {
	LoadOpa()
	tests := []struct {
		name       string
		access     *string
		chatSubs   string
		user       string
		wantEnable bool
	}{
		{name: "absent defaults to restricted allowed", chatSubs: `["allowed-user"]`, user: "allowed-user", wantEnable: true},
		{name: "absent defaults to restricted denied", chatSubs: `[]`, user: "other-user", wantEnable: false},
		{name: "disabled", access: stringPointer("disabled"), chatSubs: `["allowed-user"]`, user: "allowed-user", wantEnable: false},
		{name: "restricted allowed", access: stringPointer("restricted"), chatSubs: `["allowed-user"]`, user: "allowed-user", wantEnable: true},
		{name: "restricted denied", access: stringPointer("restricted"), chatSubs: `[]`, user: "other-user", wantEnable: false},
		{name: "all", access: stringPointer("all"), chatSubs: `[]`, user: "other-user", wantEnable: true},
		{name: "invalid falls back to restricted allowed", access: stringPointer("not-a-mode"), chatSubs: `["allowed-user"]`, user: "allowed-user", wantEnable: true},
		{name: "invalid falls back to restricted denied", access: stringPointer("not-a-mode"), chatSubs: `[]`, user: "other-user", wantEnable: false},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			unsetEnv(t, "CYCLOPS_CS_CHAT_ACCESS")
			if test.access != nil {
				t.Setenv("CYCLOPS_CS_CHAT_ACCESS", *test.access)
			}
			t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `[]`)
			t.Setenv("CYCLOPS_CS_CHAT_SUBS", test.chatSubs)
			if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
				t.Fatalf("setup dev provider: %v", err)
			}
			resetFlagsCache()

			got, err := EvalChatEnabled(context.Background(), &User{ID: test.user, AZP: "cyclops-cs-spa"})
			if err != nil {
				t.Fatalf("EvalChatEnabled() error = %v", err)
			}
			if got != test.wantEnable {
				t.Fatalf("EvalChatEnabled() = %v, want %v", got, test.wantEnable)
			}
		})
	}
}

func stringPointer(value string) *string {
	return &value
}

func TestInvalidateFeatureFlagsPreventsInFlightRefreshFromPublishingStaleData(t *testing.T) {
	resetFlagsCache()
	originalCompute := computeFlagsDataFn
	t.Cleanup(func() { computeFlagsDataFn = originalCompute })

	started := make(chan struct{})
	release := make(chan struct{})
	var calls int
	computeFlagsDataFn = func(context.Context) map[string]interface{} {
		calls++
		if calls == 1 {
			close(started)
			<-release
			return map[string]interface{}{"admin_subs": []interface{}{"stale-admin"}}
		}
		return map[string]interface{}{"admin_subs": []interface{}{"fresh-admin"}}
	}

	done := make(chan map[string]interface{}, 1)
	go func() { done <- flagsData() }()
	<-started
	InvalidateFeatureFlags()
	close(release)
	if got := <-done; !reflect.DeepEqual(got, map[string]interface{}{"admin_subs": []interface{}{"stale-admin"}}) {
		t.Fatalf("in-flight caller result = %#v", got)
	}

	flagsMu.Lock()
	cached := flagsValue
	flagsMu.Unlock()
	if cached != nil {
		t.Fatalf("stale refresh republished cache = %#v", cached)
	}

	fresh := flagsData()
	if !reflect.DeepEqual(fresh, map[string]interface{}{"admin_subs": []interface{}{"fresh-admin"}}) {
		t.Fatalf("next caller result = %#v, want fresh generation", fresh)
	}
	if calls != 2 {
		t.Fatalf("compute calls = %d, want 2", calls)
	}
	if cachedAgain := flagsData(); !reflect.DeepEqual(cachedAgain, fresh) || calls != 2 {
		t.Fatalf("cached fresh result = %#v calls=%d", cachedAgain, calls)
	}
}

// TestExportedEvalsEndToEnd wires the real path: SimpleEnvProvider →
// flagsData() → input.flags → the prepared OPA query. It guards the
// glue (flag discovery, env→key mapping, result decoding) that the
// pure-policy tests in policy_test.go don't touch.
func TestFeatureFlagAdminMiddlewareBypassesAnotherReplicasStaleAdminCache(t *testing.T) {
	LoadOpa()
	resetFlagsCache()
	originalCompute := computeFlagsDataFn
	originalFresh := computeFreshAdminFlagsFn
	t.Cleanup(func() {
		computeFlagsDataFn = originalCompute
		computeFreshAdminFlagsFn = originalFresh
	})

	// Replica B cached membership before replica A removed the administrator.
	computeFlagsDataFn = func(context.Context) map[string]interface{} {
		return map[string]interface{}{"admin_subs": []interface{}{"removed-admin"}}
	}
	if got := flagsData(); !reflect.DeepEqual(got["admin_subs"], []interface{}{"removed-admin"}) {
		t.Fatalf("stale replica cache = %#v", got)
	}

	// The shared provider now reflects replica A's committed mutation. Replica
	// B's process-local invalidator was never called.
	var freshCalls int
	computeFreshAdminFlagsFn = func(context.Context) map[string]interface{} {
		freshCalls++
		return map[string]interface{}{"admin_subs": []interface{}{}}
	}

	policy := surfacePolicies["feature-flags"]
	handler := PolicyMiddleware(policy.tree(), policy.options...)(http.HandlerFunc(func(http.ResponseWriter, *http.Request) {
		t.Fatal("removed administrator reached feature flag handler")
	}))
	request := routeRequest(http.MethodGet, "/api/admin/feature-flags", "/api/admin/feature-flags", nil, "")
	request = request.WithContext(context.WithValue(request.Context(), UserKey, &User{ID: "removed-admin", AZP: "cyclops-cs-spa"}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)

	if response.Code != http.StatusForbidden {
		t.Fatalf("status = %d, want 403; body = %s", response.Code, response.Body.String())
	}
	if freshCalls != 1 {
		t.Fatalf("fresh admin provider calls = %d, want 1", freshCalls)
	}
	if got := flagsData(); !reflect.DeepEqual(got["admin_subs"], []interface{}{"removed-admin"}) {
		t.Fatalf("unrelated TTL cache changed = %#v", got)
	}

	computeFreshAdminFlagsFn = func(context.Context) map[string]interface{} {
		freshCalls++
		return map[string]interface{}{"admin_subs": []interface{}{"current-admin"}}
	}
	allowed := PolicyMiddleware(policy.tree(), policy.options...)(http.HandlerFunc(func(w http.ResponseWriter, request *http.Request) {
		isAdmin, err := EvalIsAdminFresh(request.Context(), &User{ID: "current-admin", AZP: "cyclops-cs-spa"})
		if err != nil || !isAdmin {
			t.Fatalf("handler defense check = %v, %v", isAdmin, err)
		}
		w.WriteHeader(http.StatusNoContent)
	}))
	request = routeRequest(http.MethodGet, "/api/admin/feature-flags", "/api/admin/feature-flags", nil, "")
	request = request.WithContext(context.WithValue(request.Context(), UserKey, &User{ID: "current-admin", AZP: "cyclops-cs-spa"}))
	response = httptest.NewRecorder()
	allowed.ServeHTTP(response, request)
	if response.Code != http.StatusNoContent {
		t.Fatalf("allowed status = %d; body = %s", response.Code, response.Body.String())
	}
	if freshCalls != 2 {
		t.Fatalf("fresh provider calls after middleware and handler = %d, want 2 total", freshCalls)
	}
}

func TestExportedEvalsEndToEnd(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-sub"]`)
	t.Setenv("CYCLOPS_CS_CHAT_SUBS", `["chat-sub"]`)

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

	t.Run("EvalChatEnabled", func(t *testing.T) {
		for _, test := range []struct {
			name string
			user *User
			want bool
		}{
			{name: "admin", user: &User{ID: "admin-sub", AZP: "cyclops-cs-spa"}, want: true},
			{name: "allowlisted", user: &User{ID: "chat-sub", AZP: "cyclops-cs-spa"}, want: true},
			{name: "unlisted", user: &User{ID: "someone-else", AZP: "cyclops-cs-spa"}, want: false},
			{name: "nil", user: nil, want: false},
		} {
			t.Run(test.name, func(t *testing.T) {
				ok, err := EvalChatEnabled(ctx, test.user)
				if err != nil || ok != test.want {
					t.Fatalf("EvalChatEnabled(%v) = %v, %v; want %v, nil", test.user, ok, err, test.want)
				}
			})
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

func TestComputeFlagsDataLoadsOnlyOPAStringListFlags(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-sub"]`)
	t.Setenv("CYCLOPS_CS_USAGE_SUBS", `["usage-sub"]`)
	t.Setenv("CYCLOPS_CS_BILLING_ENABLED", "true")
	t.Setenv("CYCLOPS_CS_CHAT_ACCESS", "restricted")
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup dev provider: %v", err)
	}

	got := computeFlagsData(context.Background())
	if admins := asStrings(got["admin_subs"]); len(admins) != 1 || admins[0] != "admin-sub" {
		t.Fatalf("admin_subs = %v, want [admin-sub]", admins)
	}
	if usage := asStrings(got["usage_subs"]); len(usage) != 1 || usage[0] != "usage-sub" {
		t.Fatalf("usage_subs = %v, want [usage-sub]", usage)
	}
}

func TestComputeFlagsDataFailsClosedOnAuthorizationListTypeMismatch(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-sub"]`)
	t.Setenv("CYCLOPS_CS_CHAT_SUBS", "true")
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup dev provider: %v", err)
	}

	got := computeFlagsData(context.Background())
	if len(got) != 0 {
		t.Fatalf("computeFlagsData() = %v, want empty flags after authorization list type mismatch", got)
	}
}

func TestComputeFreshAdminFlagsFailsClosedOnTypeMismatch(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", "true")
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup dev provider: %v", err)
	}

	got := computeFreshAdminFlags(context.Background())
	if admins := asStrings(got["admin_subs"]); len(admins) != 0 {
		t.Fatalf("admin_subs = %v, want empty allowlist after type mismatch", admins)
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

func TestEvalUsagePricingReadsFlagsAndFallsBackPerValue(t *testing.T) {
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatal(err)
	}
	user := &User{ID: "user-1", AZP: "cyclops-cs-spa"}

	t.Setenv("CYCLOPS_CS_USAGE_VCPU_HOUR_PRICE_USD", "0.1")
	t.Setenv("CYCLOPS_CS_USAGE_MEMORY_GIB_HOUR_PRICE_USD", "0.2")
	pricing, err := EvalUsagePricing(context.Background(), user)
	if err != nil || pricing.VCPUHourUSD != 0.1 || pricing.MemoryGiBHourUSD != 0.2 {
		t.Fatalf("pricing = %#v, err = %v", pricing, err)
	}

	t.Setenv("CYCLOPS_CS_USAGE_VCPU_HOUR_PRICE_USD", "invalid")
	t.Setenv("CYCLOPS_CS_USAGE_MEMORY_GIB_HOUR_PRICE_USD", "0")
	pricing, err = EvalUsagePricing(context.Background(), user)
	if err == nil {
		t.Fatal("invalid pricing returned nil error")
	}
	if pricing.VCPUHourUSD != DefaultUsageVCPUHourPriceUSD || pricing.MemoryGiBHourUSD != DefaultUsageMemoryGiBHourPriceUSD {
		t.Fatalf("fallback pricing = %#v", pricing)
	}
}

func TestEvalChatEnabledFailsClosedForMalformedAllowlist(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `[]`)
	t.Setenv("CYCLOPS_CS_CHAT_SUBS", `not-json`)
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup dev provider: %v", err)
	}
	LoadOpa()
	resetFlagsCache()

	enabled, err := EvalChatEnabled(context.Background(), &User{ID: "not-json", AZP: "cyclops-cs-spa"})
	if err != nil || enabled {
		t.Fatalf("EvalChatEnabled(malformed allowlist) = %v, %v; want false, nil", enabled, err)
	}
}

func TestEvalUsageEnabledAllowsAdminsAndAllowlistedUsers(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin"]`)
	t.Setenv("CYCLOPS_CS_USAGE_SUBS", `["internal"]`)
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatal(err)
	}
	LoadOpa()
	resetFlagsCache()
	for _, tc := range []struct {
		id   string
		want bool
	}{{"admin", true}, {"internal", true}, {"other", false}} {
		got, err := EvalUsageEnabled(context.Background(), &User{ID: tc.id, AZP: "cyclops-cs-spa"})
		if err != nil || got != tc.want {
			t.Fatalf("%s: %v %v", tc.id, got, err)
		}
	}
}
