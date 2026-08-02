package provider

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestNewCyclopsClientUsesStaticAccessToken(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if got := r.Header.Get("Authorization"); got != "Bearer static-token" {
			t.Fatalf("authorization = %q, want static bearer token", got)
		}
		if r.URL.Path != "/api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools" {
			t.Fatalf("path = %q", r.URL.Path)
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"items": []any{}})
	}))
	defer server.Close()

	client, err := newCyclopsClient(server.URL, "static-token", "", "", "")
	if err != nil {
		t.Fatal(err)
	}
	defer client.Destroy()
	if _, err := client.ListPools("demo"); err != nil {
		t.Fatal(err)
	}
}

func TestNewCyclopsClientUsesOAuthCredentials(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/token" {
			clientID, clientSecret, ok := r.BasicAuth()
			if !ok || clientID != "client" || clientSecret != "secret" {
				t.Fatalf("unexpected OAuth credentials: %q %q %t", clientID, clientSecret, ok)
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "oauth-token", "expires_in": 300})
			return
		}
		if got := r.Header.Get("Authorization"); got != "Bearer oauth-token" {
			t.Fatalf("authorization = %q, want OAuth bearer token", got)
		}
		if r.URL.Path != "/api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools" {
			t.Fatalf("path = %q", r.URL.Path)
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"items": []any{}})
	}))
	defer server.Close()

	client, err := newCyclopsClient(server.URL, "", "client", "secret", server.URL+"/token")
	if err != nil {
		t.Fatal(err)
	}
	defer client.Destroy()
	if _, err := client.ListPools("demo"); err != nil {
		t.Fatal(err)
	}
}
