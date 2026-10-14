package productanalytics

import (
	"context"
	"os"
	"strings"
	"testing"

	"cyclops-cs-backend/auth"
	"github.com/trycua/cloud/pkg/featureflags"
)

func TestMain(m *testing.M) {
	// External classification requires a successfully resolved empty admin set,
	// not an unavailable provider. Keep tests local and deterministic.
	if err := os.Setenv("CYCLOPS_CS_ADMIN_SUBS", `[]`); err != nil {
		panic(err)
	}
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		panic(err)
	}
	os.Exit(m.Run())
}

func TestAdminIdentityUsesTrustedMembershipAcrossSources(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-owner"]`)
	auth.InvalidateFeatureFlags()
	t.Cleanup(auth.InvalidateFeatureFlags)
	for _, source := range []struct{ azp, principal string }{
		{"cyclops-cs-spa", auth.PrincipalTypeUser},
		{"cua-cli", auth.PrincipalTypeUser},
		{"ukey-example", auth.PrincipalTypeUserKey},
		{"github-oidc", auth.PrincipalTypeGitHubOIDC},
	} {
		user := &auth.User{ID: "admin-owner", AZP: source.azp, PrincipalType: source.principal}
		if got := ClassifyIdentity(user); got != IdentityInternal {
			t.Fatalf("admin owner via %s = %q", source.azp, got)
		}
	}
	spoofed := &auth.User{ID: "external-owner", Claims: map[string]string{"is_admin": "true", "roles": "admin"}}
	if got := ClassifyIdentity(spoofed); got != IdentityExternal {
		t.Fatalf("untrusted role claim = %q", got)
	}
	// Membership refresh must work without recreating a token or restarting.
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `[]`)
	auth.InvalidateFeatureFlags()
	if got := ClassifyIdentity(&auth.User{ID: "admin-owner"}); got != IdentityExternal {
		t.Fatalf("removed admin = %q", got)
	}
}

func TestIdentityIsUnknownWhenAdminMembershipCannotBeResolved(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `{"invalid":"not a list"}`)
	auth.InvalidateFeatureFlags()
	t.Cleanup(auth.InvalidateFeatureFlags)
	if got := ClassifyIdentity(&auth.User{ID: "owner"}); got != IdentityUnknown {
		t.Fatalf("unresolved membership = %q", got)
	}
	if got := ClassifyIdentity(&auth.User{ID: "staff", Email: "staff@trycua.com", EmailVerified: true}); got != IdentityInternal {
		t.Fatalf("verified domain remains independent evidence = %q", got)
	}
}

func TestSourceForUser(t *testing.T) {
	tests := []struct {
		name string
		user *auth.User
		want string
		ok   bool
	}{
		{name: "spa", user: &auth.User{ID: "user-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, want: SourceSPA, ok: true},
		{name: "cli", user: &auth.User{ID: "user-1", AZP: "cua-cli", PrincipalType: auth.PrincipalTypeUser}, want: SourceCLI, ok: true},
		{name: "non-user cua-cli principal", user: &auth.User{ID: "user-1", AZP: "cua-cli", PrincipalType: auth.PrincipalTypeGitHubOIDC}},
		{name: "oauth proxy", user: &auth.User{ID: "user-1", AZP: "oauth2-proxy"}, want: SourceSPA, ok: true},
		{name: "user key", user: &auth.User{ID: "user-1", AZP: "ukey-demo", PrincipalType: auth.PrincipalTypeUserKey}, want: SourceUserKey, ok: true},
		{name: "github", user: &auth.User{ID: "user-1", AZP: "github-oidc", PrincipalType: auth.PrincipalTypeGitHubOIDC}},
		{name: "missing", user: nil},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			got, ok := SourceForUser(test.user, "cyclops-cs-spa")
			if got != test.want || ok != test.ok {
				t.Fatalf("SourceForUser() = %q, %v, want %q, %v", got, ok, test.want, test.ok)
			}
		})
	}
}

func TestClassifyIdentity(t *testing.T) {
	tests := []struct {
		name string
		user *auth.User
		want IdentityClass
	}{
		{name: "missing user", user: nil, want: IdentityUnknown},
		{name: "missing stable id", user: &auth.User{Email: "person@example.test", EmailVerified: true}, want: IdentityUnknown},
		{name: "verified internal domain", user: &auth.User{ID: "user-1", Email: "founder@trycua.com", EmailVerified: true}, want: IdentityInternal},
		{name: "domain case is normalized", user: &auth.User{ID: "user-1", Email: "founder@TRYCUA.COM", EmailVerified: true}, want: IdentityInternal},
		{name: "lookalike domain is external", user: &auth.User{ID: "user-1", Email: "founder@trycua.com.evil.test", EmailVerified: true}, want: IdentityExternal},
		{name: "unverified internal-looking email is external", user: &auth.User{ID: "user-1", Email: "founder@trycua.com"}, want: IdentityExternal},
		{name: "missing email is external", user: &auth.User{ID: "user-1"}, want: IdentityExternal},
		{name: "unverified external email is external", user: &auth.User{ID: "user-1", Email: "person@example.test"}, want: IdentityExternal},
		{name: "verified external email", user: &auth.User{ID: "user-1", Email: "person@example.test", EmailVerified: true}, want: IdentityExternal},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := ClassifyIdentity(test.user); got != test.want {
				t.Fatalf("ClassifyIdentity() = %q, want %q", got, test.want)
			}
		})
	}
}

func TestPseudonymForUserIDIsDeterministicAndKeyed(t *testing.T) {
	first := PseudonymForUserID("user-1", "key-a")
	if first == "" || first != PseudonymForUserID("user-1", "key-a") {
		t.Fatalf("pseudonym is not deterministic: %q", first)
	}
	if first == PseudonymForUserID("user-1", "key-b") || first == PseudonymForUserID("user-2", "key-a") {
		t.Fatalf("pseudonym is not keyed/input-bound: %q", first)
	}
}

func TestLoginSessionKeyUsesTrustedSessionClaims(t *testing.T) {
	user := &auth.User{ID: "user-1", Claims: map[string]string{"sid": "session-1", "session_state": "legacy-session"}}
	first := loginSessionKey(user)
	if first == "" || first != loginSessionKey(user) {
		t.Fatalf("session key is not stable: %q", first)
	}
	if strings.Contains(first, "user-1") || strings.Contains(first, "session-1") {
		t.Fatalf("session key contains raw identity material: %q", first)
	}
	if first == loginSessionKey(&auth.User{ID: "user-1", Claims: map[string]string{"sid": "session-2"}}) {
		t.Fatalf("different sessions share key: %q", first)
	}
	if got := loginSessionKey(&auth.User{ID: "user-1"}); got == "" {
		t.Fatal("missing-session fallback is empty")
	}
}

func TestValidateEventRejectsUnsafeProperties(t *testing.T) {
	valid := Event{
		Name:       EventPoolCreate,
		DistinctID: "subject-1",
		Properties: map[string]any{
			"outcome":        OutcomeSuccess,
			"source":         SourceSPA,
			"principal_type": auth.PrincipalTypeUser,
			"status_code":    201,
			"error_class":    "",
		},
	}
	if err := ValidateEvent(valid); err != nil {
		t.Fatalf("ValidateEvent(valid) error = %v", err)
	}

	invalidSetOnce := valid
	invalidSetOnce.SetOnce = map[string]any{"email": "secret"}
	if err := ValidateEvent(invalidSetOnce); err == nil {
		t.Fatal("ValidateEvent() accepted unsafe set-once property")
	}

	for _, key := range []string{"error", "email", "namespace", "service", "path"} {
		t.Run(key, func(t *testing.T) {
			invalid := valid
			invalid.Properties = map[string]any{key: "secret"}
			if err := ValidateEvent(invalid); err == nil {
				t.Fatalf("ValidateEvent() accepted unsafe property %q", key)
			}
		})
	}

	attribution := Event{
		Name: EventAttributionBound, DistinctID: "subject-1",
		Properties: map[string]any{"outcome": OutcomeSuccess, "source": SourceSPA, "principal_type": auth.PrincipalTypeUser, "identity_class": IdentityExternal},
		SetOnce:    map[string]any{FirstTouchUTMCampaignProperty: "openclaw-2-launch"},
	}
	if err := ValidateEvent(attribution); err != nil {
		t.Fatalf("ValidateEvent(attribution) error = %v", err)
	}
	for _, value := range []any{"has space", "", 42} {
		invalid := attribution
		invalid.SetOnce = map[string]any{FirstTouchUTMCampaignProperty: value}
		if err := ValidateEvent(invalid); err == nil {
			t.Fatalf("ValidateEvent() accepted attribution value %#v", value)
		}
	}
}

func TestQualificationLookupDiagnosticsRequireBoundedPairedValues(t *testing.T) {
	valid := func() Event {
		return Event{Name: EventQualificationRejected, DistinctID: "synthetic-subject", Properties: map[string]any{
			"reason": "binding_lookup_failed", "qualification_lookup_stage": "claim", "qualification_error_class": "deadline_exceeded",
		}}
	}
	for stage := range allowedQualificationLookupStages {
		for class := range allowedQualificationErrorClasses {
			event := valid()
			event.Properties["qualification_lookup_stage"] = stage
			event.Properties["qualification_error_class"] = class
			if stage == "pool" {
				event.Properties["reason"] = "pool_lookup_failed"
			}
			if err := ValidateEvent(event); err != nil {
				t.Fatalf("bounded pair %s/%s rejected: %v", stage, class, err)
			}
		}
	}
	for _, tc := range []struct {
		name   string
		mutate func(*Event)
	}{
		{"raw error", func(e *Event) { e.Properties["qualification_error_class"] = "https://private.example.test/secret" }},
		{"raw stage", func(e *Event) { e.Properties["qualification_lookup_stage"] = "person@example.test" }},
		{"non-string", func(e *Event) { e.Properties["qualification_error_class"] = 403 }},
		{"missing class", func(e *Event) { delete(e.Properties, "qualification_error_class") }},
		{"missing stage", func(e *Event) { delete(e.Properties, "qualification_lookup_stage") }},
		{"empty class", func(e *Event) { e.Properties["qualification_error_class"] = "" }},
		{"wrong event", func(e *Event) { e.Name = EventQualifyingWorkload }},
		{"wrong reason", func(e *Event) { e.Properties["reason"] = "claim_mismatch" }},
		{"pool stage binding reason", func(e *Event) { e.Properties["qualification_lookup_stage"] = "pool" }},
		{"binding stage pool reason", func(e *Event) { e.Properties["reason"] = "pool_lookup_failed" }},
	} {
		t.Run(tc.name, func(t *testing.T) {
			event := valid()
			tc.mutate(&event)
			if err := ValidateEvent(event); err == nil {
				t.Fatal("invalid lookup diagnostic was accepted")
			}
		})
	}
	// Existing events without these optional diagnostics remain valid.
	event := valid()
	delete(event.Properties, "qualification_lookup_stage")
	delete(event.Properties, "qualification_error_class")
	if err := ValidateEvent(event); err != nil {
		t.Fatalf("legacy rejection invalid: %v", err)
	}
}

func TestValidatePaymentFunnelEvents(t *testing.T) {
	setupStart := Event{
		Name: EventPaymentSetupStart, DistinctID: "subject-1",
		Properties: map[string]any{
			"outcome": OutcomeSuccess, "source": SourceSPA,
			"principal_type": auth.PrincipalTypeUser, "identity_class": IdentityExternal,
			"status_code": 200,
		},
	}
	if err := ValidateEvent(setupStart); err != nil {
		t.Fatalf("ValidateEvent(setup start) error = %v", err)
	}

	gate := Event{
		Name: EventPaymentGateShown, DistinctID: "subject-1",
		Properties: map[string]any{
			"outcome": OutcomeSuccess, "source": SourceSPA,
			"principal_type": auth.PrincipalTypeUser, "identity_class": IdentityExternal,
			"resource_type": "pool", "reason": ReasonNoPaymentMethod,
		},
	}
	if err := ValidateEvent(gate); err != nil {
		t.Fatalf("ValidateEvent(payment gate) error = %v", err)
	}
	gate.Properties["reason"] = ReasonCardAdmissionRequired
	if err := ValidateEvent(gate); err != nil {
		t.Fatalf("ValidateEvent(card admission gate) error = %v", err)
	}

	invalid := []struct {
		name       string
		properties map[string]any
	}{
		{name: "missing reason", properties: map[string]any{"resource_type": "pool"}},
		{name: "missing resource", properties: map[string]any{"reason": ReasonNoPaymentMethod}},
		{name: "wrong resource", properties: map[string]any{"resource_type": "claim", "reason": ReasonNoPaymentMethod}},
		{name: "wrong reason", properties: map[string]any{"resource_type": "pool", "reason": "payment_required"}},
	}
	for _, testCase := range invalid {
		t.Run(testCase.name, func(t *testing.T) {
			event := Event{Name: EventPaymentGateShown, DistinctID: "subject-1", Properties: testCase.properties}
			if err := ValidateEvent(event); err == nil {
				t.Fatalf("ValidateEvent() accepted properties %#v", testCase.properties)
			}
		})
	}

	blocked := Event{
		Name: EventResourceBlocked, DistinctID: "subject-1",
		Properties: map[string]any{"resource_type": "pool", "reason": ReasonNoPaymentMethod},
	}
	if err := ValidateEvent(blocked); err == nil {
		t.Fatal("ValidateEvent() accepted a payment-gate-only reason on a resource block")
	}
}
