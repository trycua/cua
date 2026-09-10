package keycloak

import "testing"

func TestVerifiedUserIdentityMappers(t *testing.T) {
	t.Run("stamps verified owner evidence", func(t *testing.T) {
		mappers := verifiedUserIdentityMappers(" person@example.test ", true)
		if len(mappers) != 2 {
			t.Fatalf("mappers = %d, want 2", len(mappers))
		}
		claims := map[string]map[string]string{}
		for _, mapper := range mappers {
			if mapper.Config == nil {
				t.Fatal("mapper config is nil")
			}
			claims[(*mapper.Config)["claim.name"]] = *mapper.Config
		}
		if got := claims["user_email"]["claim.value"]; got != "person@example.test" {
			t.Fatalf("user_email = %q, want trimmed verified email", got)
		}
		if got := claims["user_email_verified"]["claim.value"]; got != "true" {
			t.Fatalf("user_email_verified = %q, want true", got)
		}
		if got := claims["user_email_verified"]["jsonType.label"]; got != "boolean" {
			t.Fatalf("user_email_verified type = %q, want boolean", got)
		}
	})

	for _, test := range []struct {
		name     string
		email    string
		verified bool
	}{
		{name: "unverified", email: "person@example.test"},
		{name: "missing email", verified: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			if got := verifiedUserIdentityMappers(test.email, test.verified); len(got) != 0 {
				t.Fatalf("mappers = %d, want 0", len(got))
			}
		})
	}
}
