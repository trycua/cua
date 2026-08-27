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
}
