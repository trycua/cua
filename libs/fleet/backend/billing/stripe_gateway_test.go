package billing

import (
	"errors"
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
	})

	if got := params.SetupIntentData.Metadata[MetadataSetupGeneration]; got != "server-generated-token" {
		t.Fatalf("setup intent generation = %q, want server-generated-token", got)
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
