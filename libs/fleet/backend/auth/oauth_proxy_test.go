package auth

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestTokenAuthMiddlewareMarksOAuthProxyEmailVerified(t *testing.T) {
	var got *User
	handler := TokenAuthMiddleware(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got = GetUser(r.Context())
		w.WriteHeader(http.StatusNoContent)
	}))
	request := httptest.NewRequest(http.MethodGet, "/api/config", nil)
	request.Header.Set("X-Auth-Request-User", "user-1")
	request.Header.Set("X-Auth-Request-Email", "person@example.test")
	handler.ServeHTTP(httptest.NewRecorder(), request)

	if got == nil || got.Email != "person@example.test" || !got.EmailVerified || got.AZP != "oauth2-proxy" {
		t.Fatalf("oauth2-proxy user = %#v", got)
	}
}
