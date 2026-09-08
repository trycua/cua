package handlers

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/productanalytics"
)

func attributionRequest(t *testing.T, body any, user *auth.User) *http.Request {
	t.Helper()
	encoded, err := json.Marshal(body)
	if err != nil {
		t.Fatal(err)
	}
	request := httptest.NewRequest(http.MethodPost, "/api/analytics/attribution", bytes.NewReader(encoded))
	return request.WithContext(context.WithValue(request.Context(), auth.UserKey, user))
}

func TestRecordFleetAttributionBindsExternalFirstTouch(t *testing.T) {
	capture := &analyticsCapture{}
	h := Handlers{Analytics: capture, AuthCfg: configAuthForAnalytics()}
	request := attributionRequest(t, fleetAttributionRecord{
		Version:    1,
		CapturedAt: time.Now().Add(-time.Minute).UnixMilli(),
		Values: map[string]string{
			"utm_source":   "x",
			"utm_medium":   "organic-social",
			"utm_campaign": "openclaw-2-launch",
		},
	}, &auth.User{ID: "subject-1", Email: "person@example.test", EmailVerified: true, AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser})
	response := httptest.NewRecorder()

	h.RecordFleetAttribution(response, request)

	if response.Code != http.StatusNoContent || len(capture.events) != 1 {
		t.Fatalf("status/events = %d/%#v", response.Code, capture.events)
	}
	event := capture.events[0]
	if event.Name != productanalytics.EventAttributionBound || event.DistinctID != "subject-1" {
		t.Fatalf("event = %#v", event)
	}
	if event.SetOnce[productanalytics.FirstTouchUTMSourceProperty] != "x" ||
		event.SetOnce[productanalytics.FirstTouchUTMMediumProperty] != "organic-social" ||
		event.SetOnce[productanalytics.FirstTouchUTMCampaignProperty] != "openclaw-2-launch" {
		t.Fatalf("set once = %#v", event.SetOnce)
	}
}

func TestRecordFleetAttributionBindsMissingEmailIdentity(t *testing.T) {
	capture := &analyticsCapture{}
	h := Handlers{Analytics: capture, AuthCfg: configAuthForAnalytics()}
	response := httptest.NewRecorder()
	h.RecordFleetAttribution(response, attributionRequest(t, fleetAttributionRecord{
		Version: 1, CapturedAt: time.Now().UnixMilli(), Values: map[string]string{"utm_source": "x"},
	}, &auth.User{ID: "subject-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}))
	if response.Code != http.StatusNoContent || len(capture.events) != 1 {
		t.Fatalf("status/events = %d/%#v", response.Code, capture.events)
	}
	if capture.events[0].Properties["identity_class"] != productanalytics.IdentityExternal {
		t.Fatalf("properties = %#v", capture.events[0].Properties)
	}
}

func TestRecordFleetAttributionExcludesVerifiedInternalIdentity(t *testing.T) {
	for _, user := range []*auth.User{
		{ID: "internal-1", Email: "person@trycua.com", EmailVerified: true, AZP: "cyclops-cs-spa"},
	} {
		capture := &analyticsCapture{}
		h := Handlers{Analytics: capture, AuthCfg: configAuthForAnalytics()}
		response := httptest.NewRecorder()
		h.RecordFleetAttribution(response, attributionRequest(t, fleetAttributionRecord{
			Version: 1, CapturedAt: time.Now().UnixMilli(), Values: map[string]string{"utm_source": "x"},
		}, user))
		if response.Code != http.StatusNoContent || len(capture.events) != 0 {
			t.Fatalf("user/status/events = %#v/%d/%#v", user, response.Code, capture.events)
		}
	}
}

func TestRecordFleetAttributionRejectsUnsafeRecords(t *testing.T) {
	now := time.Now()
	tests := []struct {
		name string
		body any
		want int
	}{
		{name: "expired", body: fleetAttributionRecord{Version: 1, CapturedAt: now.Add(-fleetAttributionTTL).UnixMilli(), Values: map[string]string{"utm_source": "x"}}, want: http.StatusBadRequest},
		{name: "future", body: fleetAttributionRecord{Version: 1, CapturedAt: now.Add(time.Minute).UnixMilli(), Values: map[string]string{"utm_source": "x"}}, want: http.StatusBadRequest},
		{name: "unknown key", body: fleetAttributionRecord{Version: 1, CapturedAt: now.UnixMilli(), Values: map[string]string{"email": "person"}}, want: http.StatusBadRequest},
		{name: "unsafe value", body: fleetAttributionRecord{Version: 1, CapturedAt: now.UnixMilli(), Values: map[string]string{"utm_source": "x post"}}, want: http.StatusBadRequest},
		{name: "oversized", body: `{"version":1,"capturedAt":1,"values":{"utm_source":"` + strings.Repeat("x", fleetAttributionBodyMaxBytes) + `"}}`, want: http.StatusRequestEntityTooLarge},
	}
	user := &auth.User{ID: "subject-1", Email: "person@example.test", EmailVerified: true, AZP: "cyclops-cs-spa"}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			capture := &analyticsCapture{}
			h := Handlers{Analytics: capture, AuthCfg: configAuthForAnalytics()}
			var request *http.Request
			if raw, ok := test.body.(string); ok {
				request = httptest.NewRequest(http.MethodPost, "/api/analytics/attribution", strings.NewReader(raw))
				request = request.WithContext(context.WithValue(request.Context(), auth.UserKey, user))
			} else {
				request = attributionRequest(t, test.body, user)
			}
			response := httptest.NewRecorder()
			h.RecordFleetAttribution(response, request)
			if response.Code != test.want || len(capture.events) != 0 {
				t.Fatalf("status/events = %d/%#v", response.Code, capture.events)
			}
		})
	}
}
