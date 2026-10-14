package handlers

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/chat"
	"github.com/trycua/cloud/pkg/featureflags"
)

func TestGetConfigReturnsEffectiveChatAccess(t *testing.T) {
	auth.LoadOpa()
	user := &auth.User{ID: "user-1", AZP: "cyclops-cs-spa"}
	tests := []struct {
		name              string
		allowed           bool
		withConversations bool
		withModel         bool
		want              bool
	}{
		{name: "available and allowed", allowed: true, withConversations: true, withModel: true, want: true},
		{name: "missing conversation store", allowed: true, withModel: true, want: false},
		{name: "missing model", allowed: true, withConversations: true, want: false},
		{name: "not allowed", allowed: false, withConversations: true, withModel: true, want: false},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			h := Handlers{
				chatAccessEvaluator: func(context.Context, *auth.User) (bool, error) {
					return test.allowed, nil
				},
			}
			if test.withConversations {
				h.Conversations = chat.NewMemoryConversationStore()
			}
			if test.withModel {
				h.Model = &fakeModel{}
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

func TestGetConfigReturnsUsageAvailabilityAndPricing(t *testing.T) {
	h := Handlers{
		Usage:                &fakeUsageProvider{},
		adminAccessEvaluator: func(context.Context, *auth.User) (bool, error) { return false, nil },
		chatAccessEvaluator:  func(context.Context, *auth.User) (bool, error) { return false, nil },
		usagePricingEvaluator: func(context.Context, *auth.User) (auth.UsagePricing, error) {
			return auth.UsagePricing{VCPUHourUSD: 0.1, MemoryGiBHourUSD: 0.2}, nil
		},
	}
	w := httptest.NewRecorder()
	h.GetConfig(w, withUser(httptest.NewRequest(http.MethodGet, "/api/config", nil), &auth.User{ID: "user", AZP: "cyclops-cs-spa"}))
	var response ConfigResponse
	if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
		t.Fatal(err)
	}
	if !response.Usage {
		t.Fatal("usage provider was not reported available")
	}
	if response.UsagePricing.VCPUHourUSD != 0.1 || response.UsagePricing.MemoryGiBHourUSD != 0.2 {
		t.Fatalf("usage pricing = %#v", response.UsagePricing)
	}
}

func TestGetConfigBypassesAnotherReplicasStaleAdminCache(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["removed-admin"]`)
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup provider: %v", err)
	}
	auth.InvalidateFeatureFlags()
	auth.LoadOpa()
	user := &auth.User{ID: "removed-admin", AZP: "cyclops-cs-spa"}
	if admin, err := auth.EvalIsAdmin(context.Background(), user); err != nil || !admin {
		t.Fatalf("prime stale replica cache: admin=%v err=%v", admin, err)
	}

	// Replica A committed removal; this process never received invalidation.
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `[]`)
	request := withUser(httptest.NewRequest(http.MethodGet, "/api/config", nil), user)
	response := httptest.NewRecorder()
	Handlers{}.GetConfig(response, request)
	if response.Code != http.StatusOK {
		t.Fatalf("status = %d; body = %s", response.Code, response.Body.String())
	}
	var config ConfigResponse
	if err := json.NewDecoder(response.Body).Decode(&config); err != nil {
		t.Fatal(err)
	}
	if config.Admin {
		t.Fatal("removed administrator remained admin through stale replica cache")
	}
}
