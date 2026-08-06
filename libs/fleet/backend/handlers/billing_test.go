package handlers

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/billing"
	"cyclops-cs-backend/config"

	"github.com/trycua/cloud/pkg/featureflags"
)

type fakeBillingService struct {
	summaryCalls           int
	summarySubject         string
	setupSubject           string
	setupOptions           billing.SetupOptions
	portalSubject          string
	portalReturnURL        string
	setupCalls             int
	portalCalls            int
	defaultCustomerID      string
	defaultPaymentMethodID string
	defaultGeneration      string
	defaultApplied         bool
	defaultCalls           int
	defaultErr             error
}

func (f *fakeBillingService) Summary(_ context.Context, subject string) (billing.Summary, error) {
	f.summaryCalls++
	f.summarySubject = subject
	return billing.Summary{}, nil
}

func (f *fakeBillingService) SetDefaultPaymentMethodForSetupGeneration(_ context.Context, customerID, paymentMethodID, generation string) (bool, error) {
	f.defaultCalls++
	f.defaultCustomerID = customerID
	f.defaultPaymentMethodID = paymentMethodID
	f.defaultGeneration = generation
	return f.defaultApplied, f.defaultErr
}

func (f *fakeBillingService) CreateSetupSession(_ context.Context, subject string, options billing.SetupOptions) (string, error) {
	f.setupCalls++
	f.setupSubject = subject
	f.setupOptions = options
	return "https://checkout.stripe.test/session", nil
}

func (f *fakeBillingService) CreatePortalSession(_ context.Context, subject, returnURL string) (string, error) {
	f.portalCalls++
	f.portalSubject = subject
	f.portalReturnURL = returnURL
	return "https://billing.stripe.test/session", nil
}

var billingAlice = &auth.User{ID: "user-alice"}

func newBillingRequest(method, target, body string, user *auth.User) *http.Request {
	var request *http.Request
	if body == "" {
		request = httptest.NewRequest(method, target, nil)
	} else {
		request = httptest.NewRequest(method, target, strings.NewReader(body))
	}
	if user != nil {
		request = withUser(request, user)
	}
	return request
}

func setBillingFlag(t *testing.T, enabled bool) {
	t.Helper()
	value := "false"
	if enabled {
		value = "true"
	}
	t.Setenv("CYCLOPS_CS_BILLING_ENABLED", value)
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup feature flag provider: %v", err)
	}
}

func TestBillingSummaryReturns503WithoutStripeConfiguration(t *testing.T) {
	setBillingFlag(t, true)
	service := &fakeBillingService{}
	h := Handlers{Billing: service}
	w := httptest.NewRecorder()

	h.GetBillingSummary(w, newBillingRequest(http.MethodGet, "/api/billing/summary", "", billingAlice))

	if w.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, want 503; body = %s", w.Code, w.Body.String())
	}
	if service.summarySubject != "" {
		t.Fatalf("summary subject = %q, want no service call", service.summarySubject)
	}
}

func TestBillingSummaryDerivesIdentityFromCurrentUser(t *testing.T) {
	setBillingFlag(t, true)
	service := &fakeBillingService{}
	h := Handlers{Billing: service, Stripe: config.StripeConfiguration{SecretKey: "sk_test"}}
	w := httptest.NewRecorder()

	h.GetBillingSummary(w, newBillingRequest(http.MethodGet, "/api/billing/summary", "", billingAlice))

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	if service.summarySubject != billingAlice.ID {
		t.Fatalf("summary subject = %q, want authenticated subject %q", service.summarySubject, billingAlice.ID)
	}
}

func TestCreateSetupSessionReturns503WithoutRedirectURLs(t *testing.T) {
	setBillingFlag(t, true)
	service := &fakeBillingService{}
	h := Handlers{
		Billing: service,
		Stripe: config.StripeConfiguration{
			SecretKey: "sk_test",
		},
	}
	w := httptest.NewRecorder()

	h.CreateBillingSetupSession(w, newBillingRequest(http.MethodPost, "/api/billing/setup-session", "", billingAlice))

	if w.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, want 503; body = %s", w.Code, w.Body.String())
	}
	if service.setupCalls != 0 {
		t.Fatalf("setup calls = %d, want 0", service.setupCalls)
	}
}

func TestCreateSetupSessionUsesAuthenticatedSubjectAndNoClientInput(t *testing.T) {
	setBillingFlag(t, true)
	service := &fakeBillingService{}
	h := Handlers{
		Billing: service,
		Stripe: config.StripeConfiguration{
			SecretKey:          "sk_test",
			CheckoutSuccessURL: "https://run.example.test/settings?setup=success",
			CheckoutCancelURL:  "https://run.example.test/settings?setup=cancelled",
		},
	}

	attacker := httptest.NewRecorder()
	h.CreateBillingSetupSession(attacker, newBillingRequest(http.MethodPost, "/api/billing/setup-session", `{"customer":"cus_attacker","payment_method":"pm_attacker"}`, billingAlice))
	if attacker.Code != http.StatusBadRequest {
		t.Fatalf("client billing input status = %d, want 400; body = %s", attacker.Code, attacker.Body.String())
	}
	if service.setupCalls != 0 {
		t.Fatalf("setup calls after client input = %d, want 0", service.setupCalls)
	}

	w := httptest.NewRecorder()
	h.CreateBillingSetupSession(w, newBillingRequest(http.MethodPost, "/api/billing/setup-session", "", billingAlice))
	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	if service.setupSubject != billingAlice.ID {
		t.Fatalf("setup subject = %q, want %q", service.setupSubject, billingAlice.ID)
	}
	var response BillingSessionResponse
	if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
		t.Fatal(err)
	}
	if response.URL != "https://checkout.stripe.test/session" {
		t.Fatalf("response URL = %q", response.URL)
	}
}

func TestCreatePortalSessionDerivesOwnedCustomerFromCurrentUser(t *testing.T) {
	setBillingFlag(t, true)
	service := &fakeBillingService{}
	h := Handlers{
		Billing: service,
		Stripe:  config.StripeConfiguration{SecretKey: "sk_test", PortalReturnURL: "https://run.example.test/billing"},
	}
	w := httptest.NewRecorder()

	h.CreateBillingPortalSession(w, newBillingRequest(http.MethodPost, "/api/billing/portal-session", "", billingAlice))

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	if service.portalSubject != billingAlice.ID {
		t.Fatalf("portal subject = %q, want %q", service.portalSubject, billingAlice.ID)
	}
	if service.portalReturnURL != "https://run.example.test/billing" {
		t.Fatalf("portal return URL = %q", service.portalReturnURL)
	}
}

func TestBillingEndpointsRejectWhenFlagDisabled(t *testing.T) {
	setBillingFlag(t, false)

	tests := []struct {
		name    string
		handle  func(Handlers, http.ResponseWriter, *http.Request)
		method  string
		path    string
		invoked func(*fakeBillingService) int
	}{
		{
			name:   "summary",
			handle: func(h Handlers, w http.ResponseWriter, r *http.Request) { h.GetBillingSummary(w, r) },
			method: http.MethodGet,
			path:   "/api/billing/summary",
			invoked: func(service *fakeBillingService) int {
				return service.summaryCalls
			},
		},
		{
			name:   "setup",
			handle: func(h Handlers, w http.ResponseWriter, r *http.Request) { h.CreateBillingSetupSession(w, r) },
			method: http.MethodPost,
			path:   "/api/billing/setup-session",
			invoked: func(service *fakeBillingService) int {
				return service.setupCalls
			},
		},
		{
			name:   "portal",
			handle: func(h Handlers, w http.ResponseWriter, r *http.Request) { h.CreateBillingPortalSession(w, r) },
			method: http.MethodPost,
			path:   "/api/billing/portal-session",
			invoked: func(service *fakeBillingService) int {
				return service.portalCalls
			},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			service := &fakeBillingService{}
			h := Handlers{
				Billing: service,
				Stripe: config.StripeConfiguration{
					SecretKey:          "sk_test",
					CheckoutSuccessURL: "https://run.example.test/settings?setup=success",
					CheckoutCancelURL:  "https://run.example.test/settings?setup=cancelled",
					PortalReturnURL:    "https://run.example.test/billing",
				},
			}
			w := httptest.NewRecorder()

			test.handle(h, w, newBillingRequest(test.method, test.path, "", billingAlice))

			if w.Code != http.StatusForbidden {
				t.Fatalf("status = %d, want 403; body = %s", w.Code, w.Body.String())
			}
			if calls := test.invoked(service); calls != 0 {
				t.Fatalf("billing service calls = %d, want 0", calls)
			}
		})
	}
}
