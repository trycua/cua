package handlers

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/billing"
)

type cardEligibilityAccounts struct {
	createdAt time.Time
	err       error
	subject   string
	calls     int
}

func (accounts *cardEligibilityAccounts) UserCreatedAt(_ context.Context, subject string) (time.Time, error) {
	accounts.calls++
	accounts.subject = subject
	return accounts.createdAt, accounts.err
}

type cardEligibilityBilling struct {
	subject string
	cards   []billing.SavedCard
	err     error
	calls   int
}

func (service *cardEligibilityBilling) AttachedCards(_ context.Context, subject string) ([]billing.SavedCard, error) {
	service.calls++
	service.subject = subject
	return service.cards, service.err
}

func (service *cardEligibilityBilling) Summary(context.Context, string) (billing.Summary, error) {
	panic("unexpected Summary call")
}
func (service *cardEligibilityBilling) SetDefaultPaymentMethodForSetupGeneration(context.Context, string, string, string) (bool, error) {
	panic("unexpected webhook call")
}
func (service *cardEligibilityBilling) CreateSetupSession(context.Context, string, billing.SetupOptions) (string, error) {
	panic("unexpected setup call")
}
func (service *cardEligibilityBilling) CreatePortalSession(context.Context, string, string) (string, error) {
	panic("unexpected portal call")
}

func stripeFactRequest(user *auth.User) *http.Request {
	request := httptest.NewRequest("POST", "/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims", nil)
	if user == nil {
		return request
	}
	return request.WithContext(context.WithValue(request.Context(), auth.UserKey, user))
}

func TestStripeCardFacts(t *testing.T) {
	stripeErr := errors.New("stripe unavailable: customer cus_secret payment method pm_secret")
	cases := []struct {
		name             string
		user             *auth.User
		cards            []billing.SavedCard
		createdAt        time.Time
		accountErr       error
		serviceErr       error
		withoutBilling   bool
		wantFacts        auth.FactSet
		wantUnavailable  bool
		wantError        bool
		wantCalls        int
		wantAccountCalls int
	}{
		{
			name:      "projects only raw expiration fields",
			user:      &auth.User{ID: "user-123"},
			createdAt: time.Date(2026, time.August, 15, 0, 0, 0, 0, time.UTC),
			cards:     []billing.SavedCard{{Brand: "visa", Last4: "4242", ExpYear: 2027, ExpMonth: 3}},
			wantFacts: auth.FactSet{"cards": []map[string]any{{
				"exp_year": int64(2027), "exp_month": int64(3),
			}}},
			wantCalls:        1,
			wantAccountCalls: 1,
		},
		{
			name:             "user created through August 14 is grandfathered without Stripe",
			user:             &auth.User{ID: "user-123"},
			createdAt:        time.Date(2026, time.August, 14, 23, 59, 59, int(time.Second-time.Nanosecond), time.UTC),
			wantFacts:        auth.FactSet{"grandfathered": true, "cards": []map[string]any{}},
			withoutBilling:   true,
			wantAccountCalls: 1,
		},
		{name: "empty cards are a definite fact at cutoff", user: &auth.User{ID: "user-123"}, createdAt: time.Date(2026, time.August, 15, 0, 0, 0, 0, time.UTC), wantFacts: auth.FactSet{"cards": []map[string]any{}}, wantCalls: 1, wantAccountCalls: 1},
		{name: "missing user is internal wiring error", wantError: true},
		{name: "empty subject is internal wiring error", user: &auth.User{}, wantError: true},
		{name: "Stripe error is unavailable", user: &auth.User{ID: "user-123"}, createdAt: time.Date(2026, time.August, 15, 0, 0, 0, 0, time.UTC), serviceErr: stripeErr, wantUnavailable: true, wantCalls: 1, wantAccountCalls: 1},
		{name: "Keycloak error is unavailable before Stripe", user: &auth.User{ID: "user-123"}, accountErr: errors.New("keycloak unavailable"), wantUnavailable: true, wantAccountCalls: 1},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			service := &cardEligibilityBilling{cards: testCase.cards, err: testCase.serviceErr}
			accounts := &cardEligibilityAccounts{createdAt: testCase.createdAt, err: testCase.accountErr}
			var billingService BillingService = service
			if testCase.withoutBilling {
				billingService = nil
			}
			provider := StripeCardFacts(Handlers{Billing: billingService, UserAccounts: accounts})
			facts, err := provider.LoadFacts(context.Background(), stripeFactRequest(testCase.user))
			var unavailable *auth.FactUnavailableError
			if testCase.wantUnavailable != errors.As(err, &unavailable) {
				t.Fatalf("unavailable error = %v, want %v; err = %v", errors.As(err, &unavailable), testCase.wantUnavailable, err)
			}
			if testCase.wantUnavailable && (strings.Contains(err.Error(), "cus_secret") || strings.Contains(err.Error(), "pm_secret")) {
				t.Fatalf("unavailable error leaked raw Stripe details: %v", err)
			}
			if testCase.wantError != (err != nil) && !testCase.wantUnavailable {
				t.Fatalf("error = %v, wantError %v", err, testCase.wantError)
			}
			if !reflect.DeepEqual(facts, testCase.wantFacts) {
				t.Fatalf("facts = %#v, want %#v", facts, testCase.wantFacts)
			}
			if service.calls != testCase.wantCalls {
				t.Fatalf("billing calls = %d, want %d", service.calls, testCase.wantCalls)
			}
			if accounts.calls != testCase.wantAccountCalls {
				t.Fatalf("account calls = %d, want %d", accounts.calls, testCase.wantAccountCalls)
			}
			if service.calls > 0 && service.subject != "user-123" {
				t.Fatalf("subject = %q, want resolved User.ID", service.subject)
			}
			if len(facts) > 2 {
				t.Fatalf("fact leaked extra data: %#v", facts)
			}
		})
	}
}
