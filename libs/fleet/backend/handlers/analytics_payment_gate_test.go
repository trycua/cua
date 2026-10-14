package handlers

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/middlewares"
	"cyclops-cs-backend/productanalytics"
)

func paymentGateRequest(body string, user *auth.User) *http.Request {
	request := httptest.NewRequest(http.MethodPost, "/api/analytics/payment-gate", strings.NewReader(body))
	if user == nil {
		return request
	}
	ctx := context.WithValue(request.Context(), auth.UserKey, user)
	ctx = context.WithValue(ctx, middlewares.ContextKey("traceId"), "trace-payment-gate")
	return request.WithContext(ctx)
}

func TestRecordFleetPaymentGateCapturesExternalGate(t *testing.T) {
	for _, reason := range []string{
		productanalytics.ReasonNoPaymentMethod,
		productanalytics.ReasonCardAdmissionRequired,
	} {
		t.Run(reason, func(t *testing.T) {
			capture := &analyticsCapture{}
			h := Handlers{Analytics: capture, AuthCfg: configAuthForAnalytics()}
			user := &auth.User{
				ID: "subject-1", Email: "person@example.test", EmailVerified: true,
				AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser,
			}
			response := httptest.NewRecorder()

			h.RecordFleetPaymentGate(response, paymentGateRequest(`{"reason":"`+reason+`"}`, user))

			if response.Code != http.StatusNoContent || len(capture.events) != 1 {
				t.Fatalf("status/events = %d/%#v", response.Code, capture.events)
			}
			event := capture.events[0]
			if event.Name != productanalytics.EventPaymentGateShown || event.DistinctID != user.ID || event.InsertID != "trace-payment-gate" {
				t.Fatalf("event = %#v", event)
			}
			wantProperties := map[string]any{
				"outcome":        productanalytics.OutcomeSuccess,
				"source":         productanalytics.SourceSPA,
				"principal_type": auth.PrincipalTypeUser,
				"identity_class": productanalytics.IdentityExternal,
				"resource_type":  "pool",
				"reason":         reason,
			}
			if len(event.Properties) != len(wantProperties) {
				t.Fatalf("properties = %#v, want %#v", event.Properties, wantProperties)
			}
			for key, want := range wantProperties {
				if got := event.Properties[key]; got != want {
					t.Fatalf("property %q = %#v, want %#v", key, got, want)
				}
			}
		})
	}
}

func TestRecordFleetPaymentGateExcludesVerifiedInternalIdentity(t *testing.T) {
	capture := &analyticsCapture{}
	h := Handlers{Analytics: capture, AuthCfg: configAuthForAnalytics()}
	response := httptest.NewRecorder()
	user := &auth.User{
		ID: "internal-1", Email: "person@trycua.com", EmailVerified: true,
		AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser,
	}

	h.RecordFleetPaymentGate(response, paymentGateRequest(`{"reason":"no_payment_method"}`, user))

	if response.Code != http.StatusNoContent || len(capture.events) != 0 {
		t.Fatalf("status/events = %d/%#v", response.Code, capture.events)
	}
}

func TestRecordFleetPaymentGatePreservesAuthenticatedPrincipalType(t *testing.T) {
	capture := &analyticsCapture{}
	h := Handlers{Analytics: capture, AuthCfg: configAuthForAnalytics()}
	response := httptest.NewRecorder()
	user := &auth.User{
		ID: "subject-1", AZP: "cyclops-cs-spa",
		PrincipalType: auth.PrincipalTypeGitHubOIDC,
	}

	h.RecordFleetPaymentGate(response, paymentGateRequest(`{"reason":"no_payment_method"}`, user))

	if response.Code != http.StatusNoContent || len(capture.events) != 1 {
		t.Fatalf("status/events = %d/%#v", response.Code, capture.events)
	}
	if got := capture.events[0].Properties["principal_type"]; got != auth.PrincipalTypeGitHubOIDC {
		t.Fatalf("principal_type = %#v, want authenticated value", got)
	}
}

func TestRecordFleetPaymentGateEnforcesAuthenticationAndSPASource(t *testing.T) {
	tests := []struct {
		name string
		user *auth.User
		want int
	}{
		{name: "missing user", want: http.StatusUnauthorized},
		{
			name: "CLI user",
			user: &auth.User{ID: "subject-1", AZP: "cua-cli", PrincipalType: auth.PrincipalTypeUser},
			want: http.StatusForbidden,
		},
		{
			name: "user key",
			user: &auth.User{ID: "subject-1", AZP: "ukey-demo", PrincipalType: auth.PrincipalTypeUserKey},
			want: http.StatusForbidden,
		},
	}
	for _, testCase := range tests {
		t.Run(testCase.name, func(t *testing.T) {
			capture := &analyticsCapture{}
			h := Handlers{Analytics: capture, AuthCfg: configAuthForAnalytics()}
			response := httptest.NewRecorder()

			h.RecordFleetPaymentGate(response, paymentGateRequest(`{"reason":"no_payment_method"}`, testCase.user))

			if response.Code != testCase.want || len(capture.events) != 0 {
				t.Fatalf("status/events = %d/%#v, want %d/no events", response.Code, capture.events, testCase.want)
			}
		})
	}
}

func TestRecordFleetPaymentGateRejectsInvalidRecords(t *testing.T) {
	tests := []struct {
		name string
		body string
		want int
	}{
		{name: "unknown reason", body: `{"reason":"payment_required"}`, want: http.StatusBadRequest},
		{name: "unknown field", body: `{"reason":"no_payment_method","email":"person@example.test"}`, want: http.StatusBadRequest},
		{name: "trailing content", body: `{"reason":"no_payment_method"}{}`, want: http.StatusBadRequest},
		{name: "oversized", body: `{"reason":"` + strings.Repeat("x", fleetPaymentGateBodyMaxBytes) + `"}`, want: http.StatusRequestEntityTooLarge},
	}
	user := &auth.User{ID: "subject-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}
	for _, testCase := range tests {
		t.Run(testCase.name, func(t *testing.T) {
			capture := &analyticsCapture{}
			h := Handlers{Analytics: capture, AuthCfg: configAuthForAnalytics()}
			response := httptest.NewRecorder()

			h.RecordFleetPaymentGate(response, paymentGateRequest(testCase.body, user))

			if response.Code != testCase.want || len(capture.events) != 0 {
				t.Fatalf("status/events = %d/%#v, want %d/no events", response.Code, capture.events, testCase.want)
			}
		})
	}
}
