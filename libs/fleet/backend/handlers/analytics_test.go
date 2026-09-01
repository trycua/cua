package handlers

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/config"
	"cyclops-cs-backend/middlewares"
	"cyclops-cs-backend/productanalytics"
)

func configAuthForAnalytics() config.AuthConfiguration {
	return config.AuthConfiguration{SPAClientID: "cyclops-cs-spa"}
}

type analyticsCapture struct{ events []productanalytics.Event }

func (capture *analyticsCapture) Capture(event productanalytics.Event) {
	capture.events = append(capture.events, event)
}

func TestRecordAnalyticsSessionCapturesSuccessfulSPALogin(t *testing.T) {
	capture := &analyticsCapture{}
	h := Handlers{Analytics: capture, AuthCfg: configAuthForAnalytics()}
	request := httptest.NewRequest(http.MethodPost, "/api/analytics/session", nil)
	ctx := context.WithValue(request.Context(), auth.UserKey, &auth.User{ID: "subject-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser})
	ctx = context.WithValue(ctx, middlewares.ContextKey("traceId"), "trace-1")
	response := httptest.NewRecorder()

	h.RecordAnalyticsSession(response, request.WithContext(ctx))

	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d", response.Code)
	}
	if len(capture.events) != 1 {
		t.Fatalf("events = %#v", capture.events)
	}
	event := capture.events[0]
	if event.Name != productanalytics.EventLoginSucceeded || event.DistinctID != "subject-1" || event.InsertID != "trace-1" {
		t.Fatalf("event = %#v", event)
	}
	if event.Properties["outcome"] != productanalytics.OutcomeSuccess || event.Properties["source"] != productanalytics.SourceSPA {
		t.Fatalf("properties = %#v", event.Properties)
	}
}

func TestRecordAnalyticsSessionRequiresUser(t *testing.T) {
	capture := &analyticsCapture{}
	h := Handlers{Analytics: capture, AuthCfg: configAuthForAnalytics()}
	response := httptest.NewRecorder()
	h.RecordAnalyticsSession(response, httptest.NewRequest(http.MethodPost, "/api/analytics/session", nil))
	if response.Code != http.StatusUnauthorized || len(capture.events) != 0 {
		t.Fatalf("status/events = %d/%#v", response.Code, capture.events)
	}
}
