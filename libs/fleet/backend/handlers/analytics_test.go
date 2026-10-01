package handlers

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/config"
	"cyclops-cs-backend/middlewares"
	"cyclops-cs-backend/productanalytics"
	"github.com/trycua/cloud/pkg/featureflags"
)

func TestMain(m *testing.M) {
	if err := os.Setenv("CYCLOPS_CS_ADMIN_SUBS", `[]`); err != nil {
		panic(err)
	}
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		panic(err)
	}
	os.Exit(m.Run())
}

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
	user := &auth.User{ID: "subject-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}
	ctx := context.WithValue(request.Context(), auth.UserKey, user)
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

func TestRecordAnalyticsSessionClassifiesAdminWithoutEmailAsInternal(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-owner"]`)
	auth.InvalidateFeatureFlags()
	t.Cleanup(auth.InvalidateFeatureFlags)
	capture := &analyticsCapture{}
	h := Handlers{Analytics: capture, AuthCfg: configAuthForAnalytics()}
	r := httptest.NewRequest(http.MethodPost, "/api/analytics/session", nil)
	user := &auth.User{ID: "admin-owner", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}
	w := httptest.NewRecorder()
	h.RecordAnalyticsSession(w, r.WithContext(context.WithValue(r.Context(), auth.UserKey, user)))
	if w.Code != http.StatusNoContent || len(capture.events) != 1 || capture.events[0].Properties["identity_class"] != productanalytics.IdentityInternal {
		t.Fatalf("admin session status/events = %d/%#v", w.Code, capture.events)
	}
}
