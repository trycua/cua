package handlers

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"cyclops-cs-backend/billing"
	"cyclops-cs-backend/config"
	"cyclops-cs-backend/productanalytics"
)

func stripeTestSignature(payload []byte, secret string, timestamp int64) string {
	mac := hmac.New(sha256.New, []byte(secret))
	_, _ = fmt.Fprintf(mac, "%d.", timestamp)
	_, _ = mac.Write(payload)
	return fmt.Sprintf("t=%d,v1=%s", timestamp, hex.EncodeToString(mac.Sum(nil)))
}

func TestBillingWebhookRejectsInvalidSignature(t *testing.T) {
	h := Handlers{
		Stripe:          config.StripeConfiguration{WebhookSecret: "whsec_test"},
		WebhookVerifier: billing.NewStripeWebhookVerifier(),
	}
	r := httptest.NewRequest(http.MethodPost, "/api/billing/webhook", strings.NewReader(`{"id":"evt_bad","type":"invoice.paid"}`))
	r.Header.Set("Stripe-Signature", "t=1,v1=invalid")
	w := httptest.NewRecorder()

	h.HandleBillingWebhook(w, r)

	if w.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400; body = %s", w.Code, w.Body.String())
	}
}

func TestBillingWebhookAcceptsValidSignatureOverRawBody(t *testing.T) {
	secret := "whsec_test"
	payload := []byte("{\n  \"id\": \"evt_valid\",\n  \"type\": \"invoice.paid\",\n  \"data\": {\"object\": {}}\n}\n")
	h := Handlers{
		Stripe:          config.StripeConfiguration{WebhookSecret: secret},
		WebhookVerifier: billing.NewStripeWebhookVerifier(),
	}
	r := httptest.NewRequest(http.MethodPost, "/api/billing/webhook", strings.NewReader(string(payload)))
	r.Header.Set("Stripe-Signature", stripeTestSignature(payload, secret, time.Now().Unix()))
	w := httptest.NewRecorder()

	h.HandleBillingWebhook(w, r)

	if w.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want 204; body = %s", w.Code, w.Body.String())
	}
}

func TestBillingWebhookRejectsOversizedRawBody(t *testing.T) {
	h := Handlers{
		Stripe:          config.StripeConfiguration{WebhookSecret: "whsec_test"},
		WebhookVerifier: billing.NewStripeWebhookVerifier(),
	}
	payload := strings.Repeat("x", stripeWebhookBodyLimit+1)
	r := httptest.NewRequest(http.MethodPost, "/api/billing/webhook", strings.NewReader(payload))
	r.Header.Set("Stripe-Signature", "t=1,v1=invalid")
	w := httptest.NewRecorder()

	h.HandleBillingWebhook(w, r)

	if w.Code != http.StatusRequestEntityTooLarge {
		t.Fatalf("status = %d, want 413; body = %s", w.Code, w.Body.String())
	}
}

type fakeWebhookVerifier struct {
	event billing.WebhookEvent
	err   error
}

func (f fakeWebhookVerifier) Verify(_ []byte, _, _ string) (billing.WebhookEvent, error) {
	return f.event, f.err
}

func runBillingWebhook(t *testing.T, service *fakeBillingService, event billing.WebhookEvent) *httptest.ResponseRecorder {
	t.Helper()
	h := Handlers{
		Billing:         service,
		Stripe:          config.StripeConfiguration{WebhookSecret: "whsec_test"},
		WebhookVerifier: fakeWebhookVerifier{event: event},
	}
	r := httptest.NewRequest(http.MethodPost, "/api/billing/webhook", strings.NewReader("{}"))
	w := httptest.NewRecorder()
	h.HandleBillingWebhook(w, r)
	return w
}

func TestBillingWebhookSetsDefaultPaymentMethodForFleetSetupIntent(t *testing.T) {
	service := &fakeBillingService{defaultApplied: true}
	response := runBillingWebhook(t, service, billing.WebhookEvent{
		Type: "setup_intent.succeeded", Purpose: billing.SetupPurpose,
		CustomerID: "cus_owned", PaymentMethodID: "pm_card", SetupGeneration: "current",
	})
	if response.Code != http.StatusNoContent || service.defaultCalls != 1 || service.defaultCustomerID != "cus_owned" || service.defaultPaymentMethodID != "pm_card" {
		t.Fatalf("default update = calls:%d customer:%q payment:%q", service.defaultCalls, service.defaultCustomerID, service.defaultPaymentMethodID)
	}
}

func TestBillingWebhookIgnoresUnrelatedSetupIntent(t *testing.T) {
	service := &fakeBillingService{}
	response := runBillingWebhook(t, service, billing.WebhookEvent{
		Type: "setup_intent.succeeded", Purpose: "other_product",
		CustomerID: "cus_owned", PaymentMethodID: "pm_card",
	})
	if response.Code != http.StatusNoContent || service.defaultCalls != 0 {
		t.Fatalf("status/calls = %d/%d, want 204/0", response.Code, service.defaultCalls)
	}
}

func TestBillingWebhookIgnoresMalformedSucceededEvent(t *testing.T) {
	service := &fakeBillingService{}
	response := runBillingWebhook(t, service, billing.WebhookEvent{
		Type: "setup_intent.succeeded", Purpose: billing.SetupPurpose,
	})
	if response.Code != http.StatusNoContent || service.defaultCalls != 0 {
		t.Fatalf("status/calls = %d/%d, want 204/0", response.Code, service.defaultCalls)
	}
}

func TestBillingWebhookReturns502WhenDefaultUpdateFails(t *testing.T) {
	service := &fakeBillingService{defaultErr: errors.New("stripe unavailable")}
	response := runBillingWebhook(t, service, billing.WebhookEvent{
		Type: "setup_intent.succeeded", Purpose: billing.SetupPurpose,
		CustomerID: "cus_owned", PaymentMethodID: "pm_card", SetupGeneration: "current",
	})
	if response.Code != http.StatusBadGateway || service.defaultCalls != 1 {
		t.Fatalf("status/calls = %d/%d, want 502/1", response.Code, service.defaultCalls)
	}
}

func TestBillingWebhookDuplicateDeliveryIsSafe(t *testing.T) {
	service := &fakeBillingService{defaultApplied: true}
	event := billing.WebhookEvent{
		Type: "setup_intent.succeeded", Purpose: billing.SetupPurpose,
		CustomerID: "cus_owned", PaymentMethodID: "pm_card", SetupGeneration: "current",
	}
	first := runBillingWebhook(t, service, event)
	second := runBillingWebhook(t, service, event)
	if first.Code != http.StatusNoContent || second.Code != http.StatusNoContent || service.defaultCalls != 2 {
		t.Fatalf("statuses/calls = %d/%d/%d, want 204/204/2", first.Code, second.Code, service.defaultCalls)
	}
}

func TestBillingWebhookIgnoresCorrectlySignedTruncatedEnvelope(t *testing.T) {
	secret := "whsec_test"
	payload := []byte(`{"id":"evt_truncated"`)
	service := &fakeBillingService{}
	h := Handlers{
		Billing:         service,
		Stripe:          config.StripeConfiguration{WebhookSecret: secret},
		WebhookVerifier: billing.NewStripeWebhookVerifier(),
	}
	r := httptest.NewRequest(http.MethodPost, "/api/billing/webhook", strings.NewReader(string(payload)))
	r.Header.Set("Stripe-Signature", stripeTestSignature(payload, secret, time.Now().Unix()))
	w := httptest.NewRecorder()

	h.HandleBillingWebhook(w, r)

	if w.Code != http.StatusNoContent || service.defaultCalls != 0 {
		t.Fatalf("status/calls = %d/%d, want 204/0", w.Code, service.defaultCalls)
	}
}

func TestBillingWebhookIgnoresStaleSetupGeneration(t *testing.T) {
	service := &fakeBillingService{defaultApplied: false}
	response := runBillingWebhook(t, service, billing.WebhookEvent{
		Type: "setup_intent.succeeded", Purpose: billing.SetupPurpose,
		CustomerID: "cus_owned", PaymentMethodID: "pm_older", SetupGeneration: "older",
	})
	if response.Code != http.StatusNoContent || service.defaultCalls != 1 || service.defaultGeneration != "older" {
		t.Fatalf("status/calls/generation = %d/%d/%q, want 204/1/older", response.Code, service.defaultCalls, service.defaultGeneration)
	}
}

func TestBillingWebhookIgnoresMissingSetupGeneration(t *testing.T) {
	service := &fakeBillingService{defaultApplied: true}
	response := runBillingWebhook(t, service, billing.WebhookEvent{
		Type: "setup_intent.succeeded", Purpose: billing.SetupPurpose,
		CustomerID: "cus_owned", PaymentMethodID: "pm_card",
	})
	if response.Code != http.StatusNoContent || service.defaultCalls != 0 {
		t.Fatalf("status/calls = %d/%d, want 204/0", response.Code, service.defaultCalls)
	}
}

func runBillingWebhookWithAnalytics(t *testing.T, service *fakeBillingService, event billing.WebhookEvent, capture *analyticsCapture) *httptest.ResponseRecorder {
	t.Helper()
	h := Handlers{Billing: service, Analytics: capture, Stripe: config.StripeConfiguration{WebhookSecret: "whsec_test"}, WebhookVerifier: fakeWebhookVerifier{event: event}}
	request := httptest.NewRequest(http.MethodPost, "/api/billing/webhook", strings.NewReader("{}"))
	response := httptest.NewRecorder()
	h.HandleBillingWebhook(response, request)
	return response
}

func TestBillingWebhookEmitsAppliedPaymentSuccess(t *testing.T) {
	service := &fakeBillingService{defaultApplied: true}
	capture := &analyticsCapture{}
	response := runBillingWebhookWithAnalytics(t, service, billing.WebhookEvent{ID: "evt_123", Type: "setup_intent.succeeded", Purpose: billing.SetupPurpose, Subject: "subject-1", Source: productanalytics.SourceSPA, CustomerID: "cus_owned", PaymentMethodID: "pm_card", SetupGeneration: "current"}, capture)
	if response.Code != http.StatusNoContent || len(capture.events) != 1 {
		t.Fatalf("status/events = %d/%#v", response.Code, capture.events)
	}
	event := capture.events[0]
	if event.Name != productanalytics.EventPaymentMethodSetup || event.DistinctID != "subject-1" || event.InsertID != "evt_123" || event.Properties["outcome"] != productanalytics.OutcomeSuccess {
		t.Fatalf("event = %#v", event)
	}
}

func TestBillingWebhookDoesNotEmitForStaleGeneration(t *testing.T) {
	service := &fakeBillingService{defaultApplied: false}
	capture := &analyticsCapture{}
	runBillingWebhookWithAnalytics(t, service, billing.WebhookEvent{ID: "evt_stale", Type: "setup_intent.succeeded", Purpose: billing.SetupPurpose, Subject: "subject-1", Source: productanalytics.SourceSPA, CustomerID: "cus_owned", PaymentMethodID: "pm_card", SetupGeneration: "old"}, capture)
	if len(capture.events) != 0 {
		t.Fatalf("events = %#v", capture.events)
	}
}

func TestBillingWebhookEmitsTrustedTerminalPaymentFailure(t *testing.T) {
	capture := &analyticsCapture{}
	response := runBillingWebhookWithAnalytics(t, &fakeBillingService{}, billing.WebhookEvent{ID: "evt_failed", Type: "setup_intent.setup_failed", Purpose: billing.SetupPurpose, Subject: "subject-1", Source: productanalytics.SourceSPA, SetupGeneration: "current"}, capture)
	if response.Code != http.StatusNoContent || len(capture.events) != 1 {
		t.Fatalf("status/events = %d/%#v", response.Code, capture.events)
	}
	event := capture.events[0]
	if event.InsertID != "evt_failed" || event.Properties["outcome"] != productanalytics.OutcomeFailure || event.Properties["error_class"] != "payment_provider" {
		t.Fatalf("event = %#v", event)
	}
}

func TestBillingWebhookMissingSubjectEmitsNothing(t *testing.T) {
	capture := &analyticsCapture{}
	runBillingWebhookWithAnalytics(t, &fakeBillingService{}, billing.WebhookEvent{ID: "evt_failed", Type: "setup_intent.setup_failed", Purpose: billing.SetupPurpose, Source: productanalytics.SourceSPA, SetupGeneration: "current"}, capture)
	if len(capture.events) != 0 {
		t.Fatalf("events = %#v", capture.events)
	}
}
