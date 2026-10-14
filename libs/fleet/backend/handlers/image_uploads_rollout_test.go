package handlers

import (
	"net/http"
	"net/http/httptest"
	"testing"

	"cyclops-cs-backend/auth"
)

func TestPresignImageUploadsAdminRolloutRoute(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-owner"]`)
	auth.InvalidateFeatureFlags()
	t.Cleanup(auth.InvalidateFeatureFlags)
	auth.LoadOpa()
	for _, subject := range []string{"admin-owner", "ordinary-owner"} {
		for _, principal := range []struct {
			name string
			user *auth.User
		}{
			{"interactive", &auth.User{ID: subject, AZP: "cyclops-cs-spa"}},
			{"cli", &auth.User{ID: subject, AZP: "cua-cli"}},
			{"user-key", &auth.User{ID: subject, AZP: "ukey-example", PrincipalType: auth.PrincipalTypeUserKey}},
			{"github", &auth.User{ID: subject, PrincipalType: auth.PrincipalTypeGitHubOIDC, AllowedNamespaces: []string{"workers"}}},
			{"namespace-key", &auth.User{ID: "service-account", AZP: "key-example", Namespace: "workers", Claims: map[string]string{"user_sub": subject}}},
		} {
			for _, namespace := range []string{"workers", "foreign"} {
				t.Run(subject+"/"+principal.name+"/"+namespace, func(t *testing.T) {
					status := http.StatusForbidden
					if namespace == "workers" {
						status = http.StatusOK
					}
					fakeK8s := newFakeK8s(status, `{"items":[]}`)
					defer fakeK8s.server.Close()
					overrideK8sClient(fakeK8s.server.Client(), fakeK8s.server.URL, "fake-sa-token")
					store := &fakeImageObjectStore{presignedURL: "https://objects.example.test/upload"}
					h := imageUploadHandlers(store)
					mux := http.NewServeMux()
					const route = "/api/image-uploads/presign"
					mux.Handle("POST "+route, auth.RouteContext(route)(auth.RouteMiddleware(route)(http.HandlerFunc(h.PresignImageUploads))))
					payload := validImageUploadRequest("rootfs")
					payload.Namespace = namespace
					response := httptest.NewRecorder()
					mux.ServeHTTP(response, withUser(jsonRequest(t, payload), principal.user))
					allowed := subject == "admin-owner" && principal.name != "namespace-key" && namespace == "workers"
					want := http.StatusForbidden
					if allowed {
						want = http.StatusOK
					}
					if response.Code != want {
						t.Fatalf("status=%d, want %d; %s", response.Code, want, response.Body.String())
					}
					if !allowed {
						assertImageObjectStoreUnused(t, store)
					} else if len(store.presignCalls) != 1 {
						t.Fatalf("presign calls=%d, want 1", len(store.presignCalls))
					}
				})
			}
		}
	}
}
