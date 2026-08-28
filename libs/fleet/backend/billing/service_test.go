package billing

import (
	"context"
	"encoding/json"
	"errors"
	"reflect"
	"strings"
	"testing"
	"time"
)

type fakeGateway struct {
	searchSubject          string
	customers              []Customer
	searchErr              error
	cards                  []SavedCard
	cardsErr               error
	listCardsCalls         int
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
	previewInvoice         *Invoice
	previewErr             error
	invoices               []Invoice
	invoicesErr            error
	invoiceCustomerID      string
	invoiceCreatedAfter    time.Time
}

func (f *fakeGateway) SearchCustomers(_ context.Context, subject string) ([]Customer, error) {
	f.searchSubject = subject
	return f.customers, f.searchErr
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
	updateErr := f.updateErr
	if updateErr != nil {
		return Customer{}, updateErr
	}
	if f.updatedCustomer.ID != "" {
		return f.updatedCustomer, nil
	}
	return Customer{ID: customerID, Metadata: metadata}, nil
}

func (f *fakeGateway) ListAttachedCards(_ context.Context, _ string) ([]SavedCard, error) {
	f.listCardsCalls++
	return f.cards, f.cardsErr
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
	setupErr := f.setupErr
	if setupErr != nil {
		return "", setupErr
	}
	return "https://checkout.stripe.test/session", nil
}

func (f *fakeGateway) CreatePortalSession(_ context.Context, request PortalSessionRequest) (string, error) {
	f.portalRequest = request
	return "https://billing.stripe.test/session", nil
}

func (f *fakeGateway) PreviewInvoice(_ context.Context, customerID string) (*Invoice, error) {
	f.invoiceCustomerID = customerID
	return f.previewInvoice, f.previewErr
}

func (f *fakeGateway) ListInvoices(_ context.Context, customerID string, createdAfter time.Time) ([]Invoice, error) {
	f.invoiceCustomerID = customerID
	f.invoiceCreatedAfter = createdAfter
	return f.invoices, f.invoicesErr
}

func TestUsageAggregatesInvoiceHistoryAndPreviewLines(t *testing.T) {
	now := time.Date(2026, time.August, 22, 12, 0, 0, 0, time.UTC)
	previousStart := time.Date(2026, time.July, 1, 0, 0, 0, 0, time.UTC)
	currentStart := time.Date(2026, time.August, 1, 0, 0, 0, 0, time.UTC)
	gateway := &fakeGateway{
		customers: []Customer{{ID: "cus_owned", Metadata: map[string]string{MetadataSubject: "subject-123"}}},
		invoices: []Invoice{{
			ID: "in_previous", Currency: "usd", Total: 3200, Status: "paid",
			PeriodStart: previousStart, PeriodEnd: currentStart,
		}},
		previewInvoice: &Invoice{
			ID: "upcoming", Currency: "usd", Total: 2500, Status: "draft",
			PeriodStart: currentStart, PeriodEnd: currentStart.AddDate(0, 1, 0),
			Lines: []InvoiceLine{
				{Description: "Linux runtime", Amount: 1500, Quantity: 20, PeriodStart: currentStart, PeriodEnd: currentStart.AddDate(0, 1, 0)},
				{Description: "Linux runtime", Amount: 500, Quantity: 5, PeriodStart: currentStart, PeriodEnd: currentStart.AddDate(0, 1, 0)},
				{Description: "Storage", Amount: 500, Quantity: 10, PeriodStart: currentStart, PeriodEnd: currentStart.AddDate(0, 1, 0)},
			},
		},
	}

	usage, err := NewService(gateway).Usage(context.Background(), "subject-123", 6, now)
	if err != nil {
		t.Fatalf("Usage() error = %v", err)
	}
	if usage.CurrentEstimate != 2500 || usage.PreviousPeriodAmount != 3200 {
		t.Fatalf("amounts = current %d previous %d", usage.CurrentEstimate, usage.PreviousPeriodAmount)
	}
	if len(usage.Trend) != 2 || !usage.Trend[1].Estimate {
		t.Fatalf("trend = %#v", usage.Trend)
	}
	if len(usage.Breakdown) != 2 || usage.Breakdown[0].Name != "Linux runtime" || usage.Breakdown[0].Amount != 2000 || usage.Breakdown[0].Quantity != 25 {
		t.Fatalf("breakdown = %#v", usage.Breakdown)
	}
	wantStart := now.AddDate(0, -6, 0)
	if !gateway.invoiceCreatedAfter.Equal(wantStart) {
		t.Fatalf("created after = %v, want %v", gateway.invoiceCreatedAfter, wantStart)
	}
}

func TestUsageReturnsEmptyDataWithoutBillingCustomer(t *testing.T) {
	now := time.Date(2026, time.August, 22, 12, 0, 0, 0, time.UTC)
	usage, err := NewService(&fakeGateway{}).Usage(context.Background(), "subject-123", 3, now)
	if err != nil {
		t.Fatalf("Usage() error = %v", err)
	}
	if usage.Currency != "usd" || len(usage.Trend) != 0 || len(usage.Breakdown) != 0 {
		t.Fatalf("usage = %#v", usage)
	}
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
			want:    `{"payment_method_present":false,"card":null,"pool_create_card_required":false}`,
		},
		{
			name: "saved card",
			summary: Summary{
				PaymentMethodPresent: true,
				Card:                 &CardSummary{Brand: "visa", Last4: "4242", ExpMonth: 12, ExpYear: 2030},
			},
			want: `{"payment_method_present":true,"card":{"brand":"visa","last4":"4242","exp_month":12,"exp_year":2030},"pool_create_card_required":false}`,
		},
		{
			name: "card required for pool creation",
			summary: Summary{
				PoolCreateCardRequired: true,
			},
			want: `{"payment_method_present":false,"card":null,"pool_create_card_required":true}`,
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
		Source:     "spa",
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
		Subject:         "subject-123",
		Source:          "spa",
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

func TestAttachedCards(t *testing.T) {
	gatewayErr := errors.New("stripe unavailable")

	cases := []struct {
		name      string
		customers []Customer
		searchErr error
		cards     []SavedCard
		cardsErr  error
		want      []SavedCard
		wantErr   error
		wantLists int
	}{
		{name: "no customer", want: []SavedCard{}},
		{name: "empty attached card list", customers: []Customer{{ID: "cus_owned", Metadata: map[string]string{MetadataSubject: "subject-123"}}}, want: []SavedCard{}, wantLists: 1},
		{name: "legacy customer is read without metadata migration", customers: []Customer{{ID: "cus_legacy", Metadata: map[string]string{LegacyMetadataSubject: "subject-123"}}}, cards: []SavedCard{{Brand: "visa", Last4: "4242", ExpMonth: 9, ExpYear: 2026}}, want: []SavedCard{{Brand: "visa", Last4: "4242", ExpMonth: 9, ExpYear: 2026}}, wantLists: 1},
		{name: "attached cards are returned without qualification", customers: []Customer{{ID: "cus_owned", Metadata: map[string]string{MetadataSubject: "subject-123"}}}, cards: []SavedCard{{ExpMonth: 1, ExpYear: 2025}, {ExpMonth: 1, ExpYear: 2027}}, want: []SavedCard{{ExpMonth: 1, ExpYear: 2025}, {ExpMonth: 1, ExpYear: 2027}}, wantLists: 1},
		{name: "customer search error", searchErr: gatewayErr, wantErr: gatewayErr},
		{name: "card listing error", customers: []Customer{{ID: "cus_owned", Metadata: map[string]string{MetadataSubject: "subject-123"}}}, cardsErr: gatewayErr, wantErr: gatewayErr, wantLists: 1},
	}
	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			gateway := &fakeGateway{customers: testCase.customers, searchErr: testCase.searchErr, cards: testCase.cards, cardsErr: testCase.cardsErr}
			got, err := NewService(gateway).AttachedCards(context.Background(), "subject-123")
			if !errors.Is(err, testCase.wantErr) || !reflect.DeepEqual(got, testCase.want) {
				t.Fatalf("AttachedCards() = %#v, %v; want %#v, %v", got, err, testCase.want, testCase.wantErr)
			}
			if gateway.createCalls != 0 {
				t.Fatalf("CreateCustomer calls = %d, want 0", gateway.createCalls)
			}
			if gateway.updatedMetadata != nil {
				t.Fatalf("UpdateCustomerMetadata called during authorization: %#v", gateway.updatedMetadata)
			}
			if gateway.listCardsCalls != testCase.wantLists {
				t.Fatalf("ListAttachedCards calls = %d, want %d", gateway.listCardsCalls, testCase.wantLists)
			}
		})
	}
}
