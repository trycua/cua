package handlers

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/config"
)

func TestGetConfigReturnsEffectiveChatAccess(t *testing.T) {
	auth.LoadOpa()
	user := &auth.User{ID: "user-1", AZP: "cyclops-cs-spa"}
	tests := []struct {
		name       string
		access     config.ChatAccessMode
		restricted bool
		want       bool
	}{
		{name: "disabled", access: config.ChatAccessDisabled, restricted: true, want: false},
		{name: "all", access: config.ChatAccessAll, restricted: false, want: true},
		{name: "restricted allowed", access: config.ChatAccessRestricted, restricted: true, want: true},
		{name: "restricted denied", access: config.ChatAccessRestricted, restricted: false, want: false},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			h := Handlers{
				ChatAccess: test.access,
				chatAccessEvaluator: func(context.Context, *auth.User) (bool, error) {
					return test.restricted, nil
				},
			}
			w := httptest.NewRecorder()
			h.GetConfig(w, withUser(httptest.NewRequest(http.MethodGet, "/api/config", nil), user))

			if w.Code != http.StatusOK {
				t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
			}
			var response ConfigResponse
			if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
				t.Fatalf("decode response: %v", err)
			}
			if response.Chat != test.want {
				t.Fatalf("chat = %v, want %v", response.Chat, test.want)
			}
		})
	}
}

func TestGetConfigReturnsEffectiveUsageAccess(t *testing.T) {
	h := Handlers{usageAccessEvaluator: func(context.Context, *auth.User) (bool, error) { return true, nil }, adminAccessEvaluator: func(context.Context, *auth.User) (bool, error) { return false, nil }}
	w := httptest.NewRecorder()
	h.GetConfig(w, withUser(httptest.NewRequest(http.MethodGet, "/api/config", nil), &auth.User{ID: "user", AZP: "cyclops-cs-spa"}))
	var r ConfigResponse
	if err := json.Unmarshal(w.Body.Bytes(), &r); err != nil {
		t.Fatal(err)
	}
	if !r.Usage {
		t.Fatal("usage false")
	}
}
