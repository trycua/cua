package auth

import (
	"strings"
	"testing"
)

func TestApplyUserKeyIdentity(t *testing.T) {
	t.Run("maps owner identity", func(t *testing.T) {
		user := &User{
			ID:            "service-account-123",
			AZP:           "ukey-test123",
			PrincipalType: PrincipalTypeUser,
			Claims: map[string]string{
				"user_sub":    "user-123",
				"user_groups": "group-a,group-b",
			},
		}

		if err := applyUserKeyIdentity(user, "ukey-"); err != nil {
			t.Fatalf("applyUserKeyIdentity() error = %v", err)
		}
		if user.ID != "user-123" {
			t.Fatalf("ID = %q, want user-123", user.ID)
		}
		if user.PrincipalType != PrincipalTypeUserKey {
			t.Fatalf("PrincipalType = %q, want %q", user.PrincipalType, PrincipalTypeUserKey)
		}
		if got := strings.Join(user.Groups, ","); got != "group-a,group-b" {
			t.Fatalf("Groups = %q, want group-a,group-b", got)
		}
	})

	t.Run("rejects missing owner identity", func(t *testing.T) {
		user := &User{
			ID:            "service-account-123",
			AZP:           "ukey-test123",
			PrincipalType: PrincipalTypeUser,
			Claims:        map[string]string{},
		}

		if err := applyUserKeyIdentity(user, "ukey-"); err == nil {
			t.Fatal("applyUserKeyIdentity() error = nil, want missing user_sub error")
		}
	})

	t.Run("leaves other clients unchanged", func(t *testing.T) {
		user := &User{
			ID:            "user-123",
			AZP:           "cua-cli",
			PrincipalType: PrincipalTypeUser,
		}

		if err := applyUserKeyIdentity(user, "ukey-"); err != nil {
			t.Fatalf("applyUserKeyIdentity() error = %v", err)
		}
		if user.ID != "user-123" || user.PrincipalType != PrincipalTypeUser {
			t.Fatalf("user changed unexpectedly: %#v", user)
		}
	})
}
