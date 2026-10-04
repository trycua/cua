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

func TestListUserKeysSerializesEmptyScopeAsArray(t *testing.T) {
	keycloakServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/realms/cyclops-cs/protocol/openid-connect/token":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"access_token":"admin-token","token_type":"Bearer","expires_in":300}`))
		case "/admin/realms/cyclops-cs/clients":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`[{"id":"key-id","clientId":"ukey-demo","name":"demo key","attributes":{"managed_by":"cyclops-cs-backend","key_type":"user","owner_sub":"user-123","scope":""}}]`))
		default:
			t.Fatalf("unexpected Keycloak request: %s %s", r.Method, r.URL.Path)
		}
	}))
	defer keycloakServer.Close()

	h := Handlers{
		Admin: keycloak.NewAdmin(keycloakServer.URL, "cyclops-cs", "admin-client", "admin-secret", "key-", "ukey-"),
	}
	r := httptest.NewRequest(http.MethodGet, "/api/user-keys", nil)
	r = withUser(r, &auth.User{ID: "user-123"})
	w := httptest.NewRecorder()

	h.ListUserKeys(w, r)

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusOK, w.Body.String())
	}
	var response ListUserKeysResponse
	if err := json.NewDecoder(w.Body).Decode(&response); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	if len(response.Keys) != 1 {
		t.Fatalf("keys = %d, want 1", len(response.Keys))
	}
	if response.Keys[0].Scope == nil {
		t.Fatalf("scope = nil, want empty array")
	}
	if len(response.Keys[0].Scope) != 0 {
		t.Fatalf("scope = %v, want empty array", response.Keys[0].Scope)
	}
}

func TestCreateUserKeyReturnsConfiguredPublicTokenURL(t *testing.T) {
	var keycloakServer *httptest.Server
	createdClaims := map[string]string{}
	keycloakServer = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/realms/cyclops-cs/protocol/openid-connect/token":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"access_token":"admin-token","token_type":"Bearer","expires_in":300}`))
		case "/admin/realms/cyclops-cs/clients":
			var request struct {
				ProtocolMappers []struct {
					Config map[string]string `json:"config"`
				} `json:"protocolMappers"`
			}
			if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
				t.Fatalf("decode Keycloak client: %v", err)
			}
			for _, mapper := range request.ProtocolMappers {
				createdClaims[mapper.Config["claim.name"]] = mapper.Config["claim.value"]
			}
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
	r = withUser(r, &auth.User{ID: "user-123", Email: "person@example.test", EmailVerified: true})
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
	if createdClaims["user_email"] != "person@example.test" || createdClaims["user_email_verified"] != "true" {
		t.Fatalf("verified owner claims = %#v", createdClaims)
	}
}
