package productanalytics

import (
	"testing"

	"cyclops-cs-backend/auth"
)

func TestSourceForUser(t *testing.T) {
	tests := []struct {
		name string
		user *auth.User
		want string
		ok   bool
	}{
		{name: "spa", user: &auth.User{ID: "user-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, want: SourceSPA, ok: true},
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
