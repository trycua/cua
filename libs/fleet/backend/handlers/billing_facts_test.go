package handlers

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/billing"
)

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
		name            string
		user            *auth.User
		cards           []billing.SavedCard
		serviceErr      error
		wantFacts       auth.FactSet
		wantUnavailable bool
		wantError       bool
		wantCalls       int
	}{
		{
			name:  "projects only raw expiration fields",
			user:  &auth.User{ID: "user-123"},
			cards: []billing.SavedCard{{Brand: "visa", Last4: "4242", ExpYear: 2027, ExpMonth: 3}},
			wantFacts: auth.FactSet{"cards": []map[string]any{{
				"exp_year": int64(2027), "exp_month": int64(3),
			}}},
			wantCalls: 1,
		},
		{name: "empty cards are a definite fact", user: &auth.User{ID: "user-123"}, wantFacts: auth.FactSet{"cards": []map[string]any{}}, wantCalls: 1},
		{name: "missing user is internal wiring error", wantError: true},
		{name: "empty subject is internal wiring error", user: &auth.User{}, wantError: true},
		{name: "Stripe error is unavailable", user: &auth.User{ID: "user-123"}, serviceErr: stripeErr, wantUnavailable: true, wantCalls: 1},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			service := &cardEligibilityBilling{cards: testCase.cards, err: testCase.serviceErr}
			provider := StripeCardFacts(Handlers{Billing: service})
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
				t.Fatalf("calls = %d, want %d", service.calls, testCase.wantCalls)
			}
			if service.calls > 0 && service.subject != "user-123" {
				t.Fatalf("subject = %q, want resolved User.ID", service.subject)
			}
			if len(facts) > 1 {
				t.Fatalf("fact leaked extra data: %#v", facts)
			}
		})
	}
}
