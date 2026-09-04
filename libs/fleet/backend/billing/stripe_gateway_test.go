package billing

import (
	"context"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"reflect"
	"testing"

	"github.com/stripe/stripe-go/v85"
)

func TestDefaultCardRetrieveParamsExpandsDefaultPaymentMethod(t *testing.T) {
	params := defaultCardRetrieveParams()
	if len(params.Expand) != 1 || params.Expand[0] == nil || *params.Expand[0] != "invoice_settings.default_payment_method" {
		t.Fatalf("Expand = %#v, want invoice_settings.default_payment_method", params.Expand)
	}
}

func TestSavedCardFromCustomer(t *testing.T) {
	tests := []struct {
		name     string
		customer *stripe.Customer
		want     SavedCard
		wantErr  error
	}{
		{
			name:     "missing invoice settings",
			customer: &stripe.Customer{},
			wantErr:  ErrPaymentMethodNotFound,
		},
		{
			name:     "missing default payment method",
			customer: &stripe.Customer{InvoiceSettings: &stripe.CustomerInvoiceSettings{}},
			wantErr:  ErrPaymentMethodNotFound,
		},
		{
			name: "non-card default payment method",
			customer: &stripe.Customer{InvoiceSettings: &stripe.CustomerInvoiceSettings{
				DefaultPaymentMethod: &stripe.PaymentMethod{Type: stripe.PaymentMethodTypeUSBankAccount},
			}},
			wantErr: ErrPaymentMethodNotFound,
		},
		{
			name: "sanitized card mapping",
			customer: &stripe.Customer{InvoiceSettings: &stripe.CustomerInvoiceSettings{
				DefaultPaymentMethod: &stripe.PaymentMethod{Card: &stripe.PaymentMethodCard{
					Brand:       stripe.PaymentMethodCardBrandVisa,
					Last4:       "4242",
					ExpMonth:    12,
					ExpYear:     2030,
					Fingerprint: "must-not-leak",
				}},
			}},
			want: SavedCard{Brand: "visa", Last4: "4242", ExpMonth: 12, ExpYear: 2030},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			got, err := savedCardFromCustomer(test.customer)
			if !errors.Is(err, test.wantErr) {
				t.Fatalf("error = %v, want %v", err, test.wantErr)
			}
			if got != test.want {
				t.Fatalf("card = %#v, want %#v", got, test.want)
			}
		})
	}
}

func TestCustomerSearchQueryUsesExactEscapedMetadata(t *testing.T) {
	for _, test := range []struct {
		name string
		key  string
		want string
	}{
		{name: "fleet", key: MetadataSubject, want: `metadata['fleet_subject']:'subject\'\\123'`},
		{name: "legacy", key: LegacyMetadataSubject, want: `metadata['cyclops_subject']:'subject\'\\123'`},
	} {
		t.Run(test.name, func(t *testing.T) {
			if got := customerSearchQuery(test.key, "subject'\\123"); got != test.want {
				t.Fatalf("query = %q, want %q", got, test.want)
			}
		})
	}
}

func TestSetupSessionParamsUseCardSetupModeAndFleetPurpose(t *testing.T) {
	params := setupSessionParams(SetupSessionRequest{
		CustomerID: "cus_owned",
		SuccessURL: "https://run.example.test/settings?setup=success",
		CancelURL:  "https://run.example.test/settings?setup=cancelled",
	})

	if params.Mode == nil || *params.Mode != string(stripe.CheckoutSessionModeSetup) {
		t.Fatalf("Mode = %v, want setup", params.Mode)
	}
	if params.Customer == nil || *params.Customer != "cus_owned" {
		t.Fatalf("Customer = %v, want owned customer", params.Customer)
	}
	if len(params.PaymentMethodTypes) != 1 || params.PaymentMethodTypes[0] == nil || *params.PaymentMethodTypes[0] != "card" {
		t.Fatalf("PaymentMethodTypes = %#v, want card only", params.PaymentMethodTypes)
	}
	if params.SetupIntentData == nil || params.SetupIntentData.Metadata["purpose"] != SetupPurpose {
		t.Fatalf("SetupIntent metadata = %#v, want fleet purpose", params.SetupIntentData)
	}
	if len(params.LineItems) != 0 {
		t.Fatalf("LineItems = %#v, want none", params.LineItems)
	}
}

func TestSetupSessionParamsCopyLatestCustomerGenerationToSetupIntent(t *testing.T) {
	params := setupSessionParams(SetupSessionRequest{
		CustomerID:      "cus_owned",
		SuccessURL:      "https://run.example.test/settings?setup=success",
		CancelURL:       "https://run.example.test/settings?setup=cancelled",
		SetupGeneration: "server-generated-token",
		Subject:         "subject-1",
		Source:          "spa",
	})

	if got := params.SetupIntentData.Metadata[MetadataSetupGeneration]; got != "server-generated-token" {
		t.Fatalf("setup intent generation = %q, want server-generated-token", got)
	}
	if got := params.SetupIntentData.Metadata[MetadataSubject]; got != "subject-1" {
		t.Fatalf("setup intent subject = %q, want subject-1", got)
	}
	if got := params.SetupIntentData.Metadata[MetadataSetupSource]; got != "spa" {
		t.Fatalf("setup intent source = %q, want spa", got)
	}
}

func TestCurrentSetupGenerationRejectsStaleAndMalformedValues(t *testing.T) {
	tests := []struct {
		name       string
		metadata   map[string]string
		generation string
		want       bool
	}{
		{name: "current", metadata: map[string]string{MetadataSetupGeneration: "current"}, generation: "current", want: true},
		{name: "stale", metadata: map[string]string{MetadataSetupGeneration: "current"}, generation: "older", want: false},
		{name: "missing customer generation", metadata: map[string]string{}, generation: "current", want: false},
		{name: "missing event generation", metadata: map[string]string{MetadataSetupGeneration: "current"}, want: false},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := currentSetupGeneration(test.metadata, test.generation); got != test.want {
				t.Fatalf("currentSetupGeneration() = %t, want %t", got, test.want)
			}
		})
	}
}

func TestDefaultPaymentMethodUpdateParamsConsumeSetupGeneration(t *testing.T) {
	params := defaultPaymentMethodUpdateParams("pm_card")
	if params.InvoiceSettings == nil || params.InvoiceSettings.DefaultPaymentMethod == nil || *params.InvoiceSettings.DefaultPaymentMethod != "pm_card" {
		t.Fatalf("invoice settings = %#v", params.InvoiceSettings)
	}
	if params.Metadata[MetadataSetupGeneration] != "" {
		t.Fatalf("generation metadata = %#v, want supported unset value", params.Metadata)
	}
}

func TestCurrentSetupGenerationIgnoresDuplicateAfterMarkerConsumption(t *testing.T) {
	metadata := map[string]string{MetadataSetupGeneration: "current"}
	if !currentSetupGeneration(metadata, "current") {
		t.Fatal("first delivery must match the current marker")
	}
	metadata[MetadataSetupGeneration] = ""
	if currentSetupGeneration(metadata, "current") {
		t.Fatal("duplicate delivery must not match a consumed marker")
	}
}

func TestListAttachedCardsFiltersAndPaginates(t *testing.T) {
	var requests []url.Values
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet || r.URL.Path != "/v1/payment_methods" {
			t.Fatalf("request = %s %s, want GET /v1/payment_methods", r.Method, r.URL.Path)
		}
		requests = append(requests, r.URL.Query())
		w.Header().Set("Content-Type", "application/json")
		if r.URL.Query().Get("starting_after") == "" {
			_, _ = io.WriteString(w, `{"object":"list","data":[{"id":"pm_1","object":"payment_method","type":"card","card":{"brand":"visa","last4":"4242","exp_month":8,"exp_year":2026}}],"has_more":true,"url":"/v1/payment_methods"}`)
			return
		}
		_, _ = io.WriteString(w, `{"object":"list","data":[{"id":"pm_2","object":"payment_method","type":"card","card":{"brand":"mastercard","last4":"4444","exp_month":1,"exp_year":2027,"fingerprint":"must-not-leak"}}],"has_more":false,"url":"/v1/payment_methods"}`)
	}))
	defer server.Close()

	backend := stripe.GetBackendWithConfig(stripe.APIBackend, &stripe.BackendConfig{URL: stripe.String(server.URL)})
	client := stripe.NewClient("sk_test", stripe.WithBackends(&stripe.Backends{API: backend, Connect: backend, Uploads: backend, MeterEvents: backend}))
	gateway := &StripeGateway{client: client}

	cards, err := gateway.ListAttachedCards(context.Background(), "cus_owned")
	if err != nil {
		t.Fatalf("ListAttachedCards() error = %v", err)
	}
	want := []SavedCard{
		{Brand: "visa", Last4: "4242", ExpMonth: 8, ExpYear: 2026},
		{Brand: "mastercard", Last4: "4444", ExpMonth: 1, ExpYear: 2027},
	}
	if !reflect.DeepEqual(cards, want) {
		t.Fatalf("cards = %#v, want %#v", cards, want)
	}
	if len(requests) != 2 {
		t.Fatalf("request count = %d, want 2", len(requests))
	}
	for i, query := range requests {
		if query.Get("customer") != "cus_owned" || query.Get("type") != "card" {
			t.Fatalf("request %d query = %v, want customer=cus_owned and type=card", i+1, query)
		}
	}
	if got := requests[1].Get("starting_after"); got != "pm_1" {
		t.Fatalf("second page starting_after = %q, want pm_1", got)
	}
}
