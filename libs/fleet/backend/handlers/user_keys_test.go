package handlers

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/config"
	"cyclops-cs-backend/keycloak"
)

func TestCreateUserKeyReturnsConfiguredPublicTokenURL(t *testing.T) {
	var keycloakServer *httptest.Server
	keycloakServer = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/realms/cyclops-cs/protocol/openid-connect/token":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"access_token":"admin-token","token_type":"Bearer","expires_in":300}`))
		case "/admin/realms/cyclops-cs/clients":
			w.Header().Set("Location", keycloakServer.URL+"/admin/realms/cyclops-cs/clients/user-key-client")
			w.WriteHeader(http.StatusCreated)
		case "/admin/realms/cyclops-cs/clients/user-key-client/client-secret":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"value":"user-key-secret"}`))
		default:
			t.Fatalf("unexpected Keycloak request: %s %s", r.Method, r.URL.Path)
		}
	}))
	defer keycloakServer.Close()

	const publicTokenURL = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
	h := Handlers{
		Admin: keycloak.NewAdmin(keycloakServer.URL, "cyclops-cs", "admin-client", "admin-secret", "key-", "ukey-"),
		KC:    config.KeycloakConfiguration{TokenURL: publicTokenURL},
	}
	r := httptest.NewRequest(http.MethodPost, "/api/user-keys", bytes.NewBufferString(`{"name":"ci-key"}`))
	r = withUser(r, &auth.User{ID: "user-123"})
	w := httptest.NewRecorder()

	h.CreateUserKey(w, r)

	if w.Code != http.StatusCreated {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusCreated, w.Body.String())
	}
	var response CreateUserKeyResponse
	if err := json.NewDecoder(w.Body).Decode(&response); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	if got := response.TokenURL; got != publicTokenURL {
		t.Fatalf("token_url = %q, want %q", got, publicTokenURL)
	}
}
