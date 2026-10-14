package auth

import (
	"net/http"
	"testing"
)

func TestImageAdminRollout(t *testing.T) {
	t.Cleanup(resetFlagsCache)
	setCardAdmissionFlags(t, false, "admin-owner")
	installCountingFacts(t, "tenant-a")
	for _, subject := range []string{"admin-owner", "ordinary-owner"} {
		userKey := &User{ID: "service-account", AZP: "ukey-example", Claims: map[string]string{"user_sub": subject}}
		if err := applyUserKeyIdentity(userKey, "ukey-"); err != nil {
			t.Fatal(err)
		}
		for _, principal := range []struct {
			name string
			user *User
		}{
			{"interactive", &User{ID: subject, AZP: "cyclops-cs-spa"}},
			{"cli", &User{ID: subject, AZP: "cua-cli"}},
			{"desktop", &User{ID: subject, AZP: "cua-desktop"}},
			{"resolved-user-key", userKey},
			{"github", &User{ID: subject, PrincipalType: PrincipalTypeGitHubOIDC, AllowedNamespaces: []string{"tenant-a"}}},
			{"namespace-key", &User{ID: "service-account", AZP: "key-example", Namespace: "tenant-a", Claims: map[string]string{"user_sub": subject}}},
			{"namespace-key-admin-sub", &User{ID: subject, AZP: "key-example", Namespace: "tenant-a"}},
		} {
			for _, namespace := range []string{"tenant-a", "tenant-b"} {
				for _, operation := range []struct{ name, method, suffix, body string }{
					{"create", "POST", "", `{"spec":{}}`},
					{"patch", "PATCH", "/image-a", `{"spec":{}}`},
					{"delete", "DELETE", "/image-a", ""},
					{"list", "GET", "", ""},
					{"get", "GET", "/image-a", ""},
					{"watch", "GET", "?watch=true", ""},
				} {
					t.Run(subject+"/"+principal.name+"/"+namespace+"/"+operation.name, func(t *testing.T) {
						path := "apis/images.cua.ai/v1alpha1/namespaces/" + namespace + "/images" + operation.suffix
						response, reached := imagePolicyResponse(t, operation.method, path, operation.body, principal.user)
						allowed := namespace == "tenant-a" && principal.user.AZP != "key-example" && (subject == "admin-owner" || operation.method == "GET")
						want := http.StatusForbidden
						if allowed {
							want = http.StatusNoContent
						}
						if response.Code != want || reached != allowed {
							t.Fatalf("status=%d reached=%v, want %d; %s", response.Code, reached, want, response.Body.String())
						}
					})
				}
			}
		}
	}
}

func TestImageRolloutRequiresResolvedAdminMembership(t *testing.T) {
	t.Cleanup(resetFlagsCache)
	setCardAdmissionFlags(t, false)
	installCountingFacts(t, "tenant-a")
	for _, admins := range []string{`[]`, `not-json`, `["someone-else"]`} {
		t.Run(admins, func(t *testing.T) {
			t.Setenv("CYCLOPS_CS_ADMIN_SUBS", admins)
			InvalidateFeatureFlags()
			response, reached := imagePolicyResponse(t, "POST", "apis/images.cua.ai/v1alpha1/namespaces/tenant-a/images", `{"spec":{}}`, &User{ID: "owner", AZP: "cua-cli"})
			if response.Code != http.StatusForbidden || reached {
				t.Fatalf("status=%d reached=%v; %s", response.Code, reached, response.Body.String())
			}
		})
	}
}
