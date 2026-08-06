package billing

import (
	"context"
	"encoding/json"
	"errors"
	"reflect"
	"strings"
	"testing"
)

type fakeGateway struct {
	searchSubject          string
	customers              []Customer
	createdMetadata        map[string]string
	createIdempotencyKey   string
	createdCustomer        Customer
	createCalls            int
	updatedMetadata        map[string]string
	updatedCustomer        Customer
	updateErr              error
	setupErr               error
	operations             []string
	defaultCard            SavedCard
	defaultCardErr         error
	defaultCustomerID      string
	defaultPaymentMethodID string
	defaultGeneration      string
	defaultApplied         bool
	setupRequest           SetupSessionRequest
	portalRequest          PortalSessionRequest
}

func (f *fakeGateway) SearchCustomers(_ context.Context, subject string) ([]Customer, error) {
	f.searchSubject = subject
	return f.customers, nil
}

func (f *fakeGateway) CreateCustomer(_ context.Context, metadata map[string]string, idempotencyKey string) (Customer, error) {
	f.createCalls++
	f.createdMetadata = metadata
	f.createIdempotencyKey = idempotencyKey
	return f.createdCustomer, nil
}

func (f *fakeGateway) UpdateCustomerMetadata(_ context.Context, customerID string, metadata map[string]string) (Customer, error) {
	f.operations = append(f.operations, "publish")
	f.updatedMetadata = metadata
	if f.updateErr != nil {
		return Customer{}, f.updateErr
	}
	if f.updatedCustomer.ID != "" {
		return f.updatedCustomer, nil
	}
	return Customer{ID: customerID, Metadata: metadata}, nil
}

func (f *fakeGateway) GetDefaultCard(_ context.Context, _ string) (SavedCard, error) {
	return f.defaultCard, f.defaultCardErr
}

func (f *fakeGateway) SetDefaultPaymentMethodForSetupGeneration(_ context.Context, customerID, paymentMethodID, generation string) (bool, error) {
	f.defaultCustomerID = customerID
	f.defaultPaymentMethodID = paymentMethodID
	f.defaultGeneration = generation
	return f.defaultApplied, nil
}

func (f *fakeGateway) CreateSetupSession(_ context.Context, request SetupSessionRequest) (string, error) {
	f.operations = append(f.operations, "create")
	f.setupRequest = request
	if f.setupErr != nil {
		return "", f.setupErr
	}
	return "https://checkout.stripe.test/session", nil
}

func (f *fakeGateway) CreatePortalSession(_ context.Context, request PortalSessionRequest) (string, error) {
	f.portalRequest = request
	return "https://billing.stripe.test/session", nil
}

func TestFindOrCreateCustomerUsesExactControlledMetadata(t *testing.T) {
	gateway := &fakeGateway{
		customers: []Customer{{
			ID: "cus_wrong_subject",
			Metadata: map[string]string{
				MetadataSubject: "subject-12",
			},
		}},
		createdCustomer: Customer{ID: "cus_created"},
	}
	service := NewService(gateway)

	customer, err := service.FindOrCreateCustomer(context.Background(), "subject-123")
	if err != nil {
		t.Fatalf("FindOrCreateCustomer() error = %v", err)
	}
	if customer.ID != "cus_created" {
		t.Fatalf("customer ID = %q, want cus_created", customer.ID)
	}
	if gateway.searchSubject != "subject-123" {
		t.Fatalf("search subject = %q, want exact subject", gateway.searchSubject)
	}
	wantMetadata := map[string]string{
		MetadataSubject: "subject-123",
	}
	if !reflect.DeepEqual(gateway.createdMetadata, wantMetadata) {
		t.Fatalf("created metadata = %#v, want %#v", gateway.createdMetadata, wantMetadata)
	}
}

func TestFindOrCreateCustomerUsesFleetMetadataAndIdempotencyKey(t *testing.T) {
	gateway := &fakeGateway{createdCustomer: Customer{ID: "cus_created"}}
	service := NewService(gateway)

	customer, err := service.FindOrCreateCustomer(context.Background(), "subject-123")
	if err != nil {
		t.Fatalf("FindOrCreateCustomer() error = %v", err)
	}
	if customer.ID != "cus_created" {
		t.Fatalf("customer ID = %q, want cus_created", customer.ID)
	}
	if got := gateway.createdMetadata[MetadataSubject]; got != "subject-123" {
		t.Fatalf("fleet metadata = %q, want subject-123", got)
	}
	if _, exists := gateway.createdMetadata[LegacyMetadataSubject]; exists {
		t.Fatal("new customer must not write legacy cyclops metadata")
	}
	if !strings.HasPrefix(gateway.createIdempotencyKey, CustomerIdempotencyPrefix) {
		t.Fatalf("idempotency key = %q, want fleet prefix", gateway.createIdempotencyKey)
	}
}

func TestFindCustomerMigratesLegacySubjectMetadata(t *testing.T) {
	gateway := &fakeGateway{customers: []Customer{{
		ID:       "cus_legacy",
		Metadata: map[string]string{LegacyMetadataSubject: "subject-123"},
	}}}
	service := NewService(gateway)

	customer, err := service.FindCustomer(context.Background(), "subject-123")
	if err != nil {
		t.Fatalf("FindCustomer() error = %v", err)
	}
	if customer.ID != "cus_legacy" {
		t.Fatalf("customer ID = %q, want cus_legacy", customer.ID)
	}
	if got := gateway.updatedMetadata[MetadataSubject]; got != "subject-123" {
		t.Fatalf("updated fleet metadata = %q, want subject-123", got)
	}
	if gateway.createCalls != 0 {
		t.Fatalf("CreateCustomer calls = %d, want 0", gateway.createCalls)
	}
}

func TestFindCustomerSkipsConflictingLegacySubjectMetadata(t *testing.T) {
	gateway := &fakeGateway{customers: []Customer{{
		ID: "cus_other_owner",
		Metadata: map[string]string{
			MetadataSubject:       "other",
			LegacyMetadataSubject: "subject-123",
		},
	}}}
	service := NewService(gateway)

	customer, err := service.FindCustomer(context.Background(), "subject-123")
	if !errors.Is(err, ErrCustomerNotFound) {
		t.Fatalf("FindCustomer() customer/error = %#v/%v, want no customer/%v", customer, err, ErrCustomerNotFound)
	}
	if customer.ID != "" {
		t.Fatalf("customer = %#v, want no customer", customer)
	}
	if gateway.updatedMetadata != nil {
		t.Fatalf("updated metadata = %#v, want no update", gateway.updatedMetadata)
	}
}

func TestSummaryReturnsNoPaymentMethodWhenCustomerDoesNotExist(t *testing.T) {
	service := NewService(&fakeGateway{})

	summary, err := service.Summary(context.Background(), "subject-123")
	if err != nil {
		t.Fatalf("Summary() error = %v", err)
	}
	if summary.PaymentMethodPresent || summary.Card != nil {
		t.Fatalf("summary = %#v, want no payment method", summary)
	}
}

func TestSummaryDoesNotTreatCustomerExistenceAsSavedCard(t *testing.T) {
	gateway := &fakeGateway{
		customers:      []Customer{{ID: "cus_owned", Metadata: map[string]string{MetadataSubject: "subject-123"}}},
		defaultCardErr: ErrPaymentMethodNotFound,
	}
	service := NewService(gateway)

	summary, err := service.Summary(context.Background(), "subject-123")
	if err != nil {
		t.Fatalf("Summary() error = %v", err)
	}
	if summary.PaymentMethodPresent || summary.Card != nil {
		t.Fatalf("summary = %#v, want abandoned setup to remain cardless", summary)
	}
}

func TestSummaryReturnsSanitizedDefaultCard(t *testing.T) {
	gateway := &fakeGateway{
		customers:   []Customer{{ID: "cus_owned", Metadata: map[string]string{MetadataSubject: "subject-123"}}},
		defaultCard: SavedCard{Brand: "visa", Last4: "4242", ExpMonth: 12, ExpYear: 2030},
	}
	service := NewService(gateway)

	summary, err := service.Summary(context.Background(), "subject-123")
	if err != nil {
		t.Fatalf("Summary() error = %v", err)
	}
	if !summary.PaymentMethodPresent || summary.Card == nil {
		t.Fatalf("summary = %#v, want saved card", summary)
	}
	if *summary.Card != (CardSummary{Brand: "visa", Last4: "4242", ExpMonth: 12, ExpYear: 2030}) {
		t.Fatalf("card = %#v", *summary.Card)
	}
}

func TestSummaryJSONContract(t *testing.T) {
	tests := []struct {
		name    string
		summary Summary
		want    string
	}{
		{
			name:    "cardless",
			summary: Summary{},
			want:    `{"payment_method_present":false,"card":null}`,
		},
		{
			name: "saved card",
			summary: Summary{
				PaymentMethodPresent: true,
				Card:                 &CardSummary{Brand: "visa", Last4: "4242", ExpMonth: 12, ExpYear: 2030},
			},
			want: `{"payment_method_present":true,"card":{"brand":"visa","last4":"4242","exp_month":12,"exp_year":2030}}`,
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			encoded, err := json.Marshal(test.summary)
			if err != nil {
				t.Fatalf("Marshal() error = %v", err)
			}
			if string(encoded) != test.want {
				t.Fatalf("JSON = %s, want %s", encoded, test.want)
			}
		})
	}
}

func TestCreateSetupSessionUsesServerRequestAndOwnedCustomer(t *testing.T) {
	gateway := &fakeGateway{createdCustomer: Customer{ID: "cus_created"}}
	service := NewService(gateway)

	url, err := service.CreateSetupSession(context.Background(), "subject-123", SetupOptions{
		SuccessURL: "https://run.example.test/billing?checkout=success",
		CancelURL:  "https://run.example.test/billing?checkout=cancelled",
	})
	if err != nil {
		t.Fatalf("CreateSetupSession() error = %v", err)
	}
	if url != "https://checkout.stripe.test/session" {
		t.Fatalf("url = %q", url)
	}
	want := SetupSessionRequest{
		CustomerID:      "cus_created",
		SuccessURL:      "https://run.example.test/billing?checkout=success",
		CancelURL:       "https://run.example.test/billing?checkout=cancelled",
		SetupGeneration: gateway.updatedMetadata[MetadataSetupGeneration],
	}
	if want.SetupGeneration == "" || !reflect.DeepEqual(gateway.setupRequest, want) {
		t.Fatalf("setup request = %#v, want %#v", gateway.setupRequest, want)
	}
}

func TestCreatePortalSessionUsesOnlyOwnedExistingCustomer(t *testing.T) {
	gateway := &fakeGateway{customers: []Customer{
		{ID: "cus_attacker", Metadata: map[string]string{MetadataSubject: "other"}},
		{ID: "cus_owned", Metadata: map[string]string{MetadataSubject: "subject-123"}},
	}}
	service := NewService(gateway)

	_, err := service.CreatePortalSession(context.Background(), "subject-123", "https://run.example.test/billing")
	if err != nil {
		t.Fatalf("CreatePortalSession() error = %v", err)
	}
	want := PortalSessionRequest{CustomerID: "cus_owned", ReturnURL: "https://run.example.test/billing"}
	if !reflect.DeepEqual(gateway.portalRequest, want) {
		t.Fatalf("portal request = %#v, want %#v", gateway.portalRequest, want)
	}
}

func TestSetDefaultPaymentMethodForSetupGenerationDelegatesToGateway(t *testing.T) {
	gateway := &fakeGateway{defaultApplied: true}
	service := NewService(gateway)

	applied, err := service.SetDefaultPaymentMethodForSetupGeneration(context.Background(), "cus_owned", "pm_card", "current")
	if err != nil {
		t.Fatalf("SetDefaultPaymentMethodForSetupGeneration() error = %v", err)
	}
	if !applied {
		t.Fatal("default update was not applied")
	}
	if gateway.defaultCustomerID != "cus_owned" || gateway.defaultPaymentMethodID != "pm_card" || gateway.defaultGeneration != "current" {
		t.Fatalf("default update = %q/%q/%q", gateway.defaultCustomerID, gateway.defaultPaymentMethodID, gateway.defaultGeneration)
	}
}

func TestCreateSetupSessionStoresLatestOpaqueGeneration(t *testing.T) {
	gateway := &fakeGateway{createdCustomer: Customer{ID: "cus_owned"}}
	service := NewService(gateway)

	_, err := service.CreateSetupSession(context.Background(), "subject-123", SetupOptions{
		SuccessURL: "https://run.example.test/settings?setup=success",
		CancelURL:  "https://run.example.test/settings?setup=cancelled",
	})
	if err != nil {
		t.Fatalf("CreateSetupSession() error = %v", err)
	}
	generation := gateway.updatedMetadata[MetadataSetupGeneration]
	if generation == "" {
		t.Fatal("latest setup generation must be server-generated and stored on the customer")
	}
	if gateway.setupRequest.SetupGeneration != generation {
		t.Fatalf("setup generation = %q, want customer generation", gateway.setupRequest.SetupGeneration)
	}
}

func TestCreateSetupSessionDoesNotPublishGenerationWhenCreationFails(t *testing.T) {
	gateway := &fakeGateway{customers: []Customer{{ID: "cus_owned", Metadata: map[string]string{MetadataSubject: "subject-123", MetadataSetupGeneration: "previous"}}}, setupErr: errors.New("stripe unavailable")}
	_, err := NewService(gateway).CreateSetupSession(context.Background(), "subject-123", SetupOptions{})
	if err == nil || gateway.updatedMetadata != nil || !reflect.DeepEqual(gateway.operations, []string{"create"}) {
		t.Fatalf("error/metadata/operations = %v/%#v/%#v", err, gateway.updatedMetadata, gateway.operations)
	}
}

func TestCreateSetupSessionReturnsNoURLWhenGenerationPublishFails(t *testing.T) {
	gateway := &fakeGateway{customers: []Customer{{ID: "cus_owned", Metadata: map[string]string{MetadataSubject: "subject-123"}}}, updateErr: errors.New("stripe unavailable")}
	url, err := NewService(gateway).CreateSetupSession(context.Background(), "subject-123", SetupOptions{})
	if err == nil || url != "" || !reflect.DeepEqual(gateway.operations, []string{"create", "publish"}) {
		t.Fatalf("url/error/operations = %q/%v/%#v", url, err, gateway.operations)
	}
}
