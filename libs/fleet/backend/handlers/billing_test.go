package handlers

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/billing"
	"cyclops-cs-backend/config"
	"cyclops-cs-backend/productanalytics"

	"github.com/trycua/cloud/pkg/featureflags"
)

type fakeBillingService struct {
	summaryCalls           int
	summarySubject         string
	attachedCards          []billing.SavedCard
	attachedErr            error
	attachedCalls          int
	setupSubject           string
	setupOptions           billing.SetupOptions
	portalSubject          string
	portalReturnURL        string
	setupCalls             int
	setupErr               error
	portalCalls            int
	defaultCustomerID      string
	defaultPaymentMethodID string
	defaultGeneration      string
	defaultApplied         bool
	defaultCalls           int
	defaultErr             error
	usageMonths            int
	usageSubject           string
	usageResponse          billing.Usage
	usageErr               error
}

func (f *fakeBillingService) AttachedCards(context.Context, string) ([]billing.SavedCard, error) {
	f.attachedCalls++
	attachedErr := f.attachedErr
	if attachedErr != nil {
		return nil, attachedErr
	}
	if f.attachedCards == nil {
		return []billing.SavedCard{}, nil
	}
	return f.attachedCards, nil
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
	setupErr := f.setupErr
	if setupErr != nil {
		return "", setupErr
	}
	return "https://checkout.stripe.test/session", nil
}

func (f *fakeBillingService) CreatePortalSession(_ context.Context, subject, returnURL string) (string, error) {
	f.portalCalls++
	f.portalSubject = subject
	f.portalReturnURL = returnURL
	return "https://billing.stripe.test/session", nil
}

func (f *fakeBillingService) Usage(_ context.Context, subject string, months int, _ time.Time) (billing.Usage, error) {
	f.usageSubject = subject
	f.usageMonths = months
	return f.usageResponse, f.usageErr
}

var billingAlice = &auth.User{ID: "user-alice", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}

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

func TestGetBillingUsageUsesValidatedHistoryWindow(t *testing.T) {
	setBillingFlag(t, true)
	service := &fakeBillingService{usageResponse: billing.Usage{Currency: "usd", Trend: []billing.UsagePoint{}, Breakdown: []billing.UsageBreakdownItem{}}}
	h := Handlers{Billing: service, Stripe: config.StripeConfiguration{SecretKey: "sk_test"}}
	w := httptest.NewRecorder()

	h.GetBillingUsage(w, newBillingRequest(http.MethodGet, "/api/billing/usage?months=12", "", billingAlice))

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	if service.usageSubject != billingAlice.ID || service.usageMonths != 12 {
		t.Fatalf("usage call = subject %q months %d", service.usageSubject, service.usageMonths)
	}
}

func TestGetBillingUsageRejectsUnsupportedHistoryWindow(t *testing.T) {
	setBillingFlag(t, true)
	service := &fakeBillingService{}
	h := Handlers{Billing: service, Stripe: config.StripeConfiguration{SecretKey: "sk_test"}}
	w := httptest.NewRecorder()

	h.GetBillingUsage(w, newBillingRequest(http.MethodGet, "/api/billing/usage?months=9", "", billingAlice))

	if w.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400; body = %s", w.Code, w.Body.String())
	}
	if service.usageMonths != 0 {
		t.Fatalf("usage months = %d, want no call", service.usageMonths)
	}
}

func TestBillingSummaryReportsPoolCreateCardRequired(t *testing.T) {
	postCutoff := time.Date(2026, time.August, 15, 0, 0, 0, 0, time.UTC)
	preCutoff := time.Date(2026, time.August, 1, 0, 0, 0, 0, time.UTC)
	nextYear := int64(time.Now().UTC().Year() + 1)
	cases := []struct {
		name            string
		cardFlag        bool
		adminSubs       string
		createdAt       time.Time
		cards           []billing.SavedCard
		cardsErr        error
		want            bool
		wantStripeCalls int
	}{
		{name: "card flag off", cardFlag: false, createdAt: postCutoff, want: false},
		{name: "admin exempt", cardFlag: true, adminSubs: `["` + billingAlice.ID + `"]`, createdAt: postCutoff, want: false},
		{name: "grandfathered user", cardFlag: true, createdAt: preCutoff, want: false},
		{name: "no card", cardFlag: true, createdAt: postCutoff, want: true, wantStripeCalls: 1},
		{name: "qualifying card", cardFlag: true, createdAt: postCutoff, cards: []billing.SavedCard{{Brand: "visa", Last4: "4242", ExpYear: nextYear, ExpMonth: 1}}, want: false, wantStripeCalls: 1},
		{name: "expired card", cardFlag: true, createdAt: postCutoff, cards: []billing.SavedCard{{Brand: "visa", Last4: "4242", ExpYear: 2020, ExpMonth: 1}}, want: true, wantStripeCalls: 1},
		{name: "stripe outage fails open", cardFlag: true, createdAt: postCutoff, cardsErr: errors.New("stripe unavailable"), want: false, wantStripeCalls: 1},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			cardFlag := "false"
			if testCase.cardFlag {
				cardFlag = "true"
			}
			adminSubs := testCase.adminSubs
			if adminSubs == "" {
				adminSubs = `[]`
			}
			t.Setenv("CYCLOPS_CS_REQUIRE_CARD_FOR_CUSTOM_RESOURCE_CREATION", cardFlag)
			t.Setenv("CYCLOPS_CS_ADMIN_SUBS", adminSubs)
			t.Setenv("CYCLOPS_CS_CARD_REQUIREMENT_EXEMPT_SUBS", `[]`)
			setBillingFlag(t, true)
			auth.InvalidateFeatureFlags()
			auth.LoadOpa()
			t.Cleanup(auth.InvalidateFeatureFlags)

			service := &fakeBillingService{attachedCards: testCase.cards, attachedErr: testCase.cardsErr}
			accounts := &cardEligibilityAccounts{createdAt: testCase.createdAt}
			h := Handlers{Billing: service, UserAccounts: accounts, Stripe: config.StripeConfiguration{SecretKey: "sk_test"}}
			w := httptest.NewRecorder()

			h.GetBillingSummary(w, newBillingRequest(http.MethodGet, "/api/billing/summary", "", billingAlice))

			if w.Code != http.StatusOK {
				t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
			}
			var summary billing.Summary
			if err := json.Unmarshal(w.Body.Bytes(), &summary); err != nil {
				t.Fatalf("decode summary: %v; body = %s", err, w.Body.String())
			}
			if summary.PoolCreateCardRequired != testCase.want {
				t.Fatalf("pool_create_card_required = %v, want %v", summary.PoolCreateCardRequired, testCase.want)
			}
			if service.attachedCalls != testCase.wantStripeCalls {
				t.Fatalf("attached card lookups = %d, want %d", service.attachedCalls, testCase.wantStripeCalls)
			}
		})
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
		AuthCfg: config.AuthConfiguration{SPAClientID: "cyclops-cs-spa"},
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
	if service.setupOptions.Source != productanalytics.SourceSPA {
		t.Fatalf("setup source = %q, want spa", service.setupOptions.Source)
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

func TestCreateSetupSessionProviderFailureEmitsFailureEvent(t *testing.T) {
	setBillingFlag(t, true)
	service := &fakeBillingService{setupErr: errors.New("stripe unavailable")}
	capture := &analyticsCapture{}
	h := Handlers{
		Billing:   service,
		Analytics: capture,
		AuthCfg:   config.AuthConfiguration{SPAClientID: "cyclops-cs-spa"},
		Stripe:    config.StripeConfiguration{SecretKey: "sk_test", CheckoutSuccessURL: "https://run.example.test/success", CheckoutCancelURL: "https://run.example.test/cancel"},
	}
	response := httptest.NewRecorder()
	h.CreateBillingSetupSession(response, newBillingRequest(http.MethodPost, "/api/billing/setup-session", "", billingAlice))
	if response.Code != http.StatusBadGateway || len(capture.events) != 1 {
		t.Fatalf("status/events = %d/%#v", response.Code, capture.events)
	}
	event := capture.events[0]
	if event.Name != productanalytics.EventPaymentMethodSetup || event.DistinctID != billingAlice.ID || event.Properties["outcome"] != productanalytics.OutcomeFailure || event.Properties["error_class"] != "payment_provider" {
		t.Fatalf("event = %#v", event)
	}
}
