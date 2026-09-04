package billing

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/stripe/stripe-go/v85"
)

type StripeGateway struct {
	client *stripe.Client
}

func NewStripeGateway(secretKey string) *StripeGateway {
	return &StripeGateway{client: stripe.NewClient(secretKey)}
}

func escapeSearchValue(value string) string {
	value = strings.ReplaceAll(value, `\`, `\\`)
	return strings.ReplaceAll(value, `'`, `\'`)
}

func customerSearchQuery(metadataKey, subject string) string {
	return fmt.Sprintf("metadata['%s']:'%s'", metadataKey, escapeSearchValue(subject))
}

func (g *StripeGateway) searchCustomersByMetadata(ctx context.Context, metadataKey, subject string) ([]Customer, error) {
	params := &stripe.CustomerSearchParams{
		SearchParams: stripe.SearchParams{
			Query: customerSearchQuery(metadataKey, subject),
			Limit: stripe.Int64(10),
		},
	}
	customers := make([]Customer, 0)
	for customer, err := range g.client.V1Customers.Search(ctx, params).All(ctx) {
		if err != nil {
			return nil, err
		}
		customers = append(customers, Customer{ID: customer.ID, Metadata: customer.Metadata})
	}
	return customers, nil
}

func (g *StripeGateway) SearchCustomers(ctx context.Context, subject string) ([]Customer, error) {
	byID := map[string]Customer{}
	for _, key := range []string{MetadataSubject, LegacyMetadataSubject} {
		customers, err := g.searchCustomersByMetadata(ctx, key, subject)
		if err != nil {
			return nil, err
		}
		for _, customer := range customers {
			byID[customer.ID] = customer
		}
	}
	customers := make([]Customer, 0, len(byID))
	for _, customer := range byID {
		customers = append(customers, customer)
	}
	return customers, nil
}

func (g *StripeGateway) CreateCustomer(ctx context.Context, metadata map[string]string, idempotencyKey string) (Customer, error) {
	params := &stripe.CustomerCreateParams{Metadata: metadata}
	params.SetIdempotencyKey(idempotencyKey)
	customer, err := g.client.V1Customers.Create(ctx, params)
	if err != nil {
		return Customer{}, err
	}
	return Customer{ID: customer.ID, Metadata: customer.Metadata}, nil
}

func (g *StripeGateway) UpdateCustomerMetadata(ctx context.Context, customerID string, metadata map[string]string) (Customer, error) {
	params := &stripe.CustomerUpdateParams{Metadata: metadata}
	customer, err := g.client.V1Customers.Update(ctx, customerID, params)
	if err != nil {
		return Customer{}, err
	}
	return Customer{ID: customer.ID, Metadata: customer.Metadata}, nil
}

func defaultCardRetrieveParams() *stripe.CustomerRetrieveParams {
	params := &stripe.CustomerRetrieveParams{}
	params.AddExpand("invoice_settings.default_payment_method")
	return params
}

func savedCardFromCustomer(customer *stripe.Customer) (SavedCard, error) {
	if customer == nil ||
		customer.InvoiceSettings == nil ||
		customer.InvoiceSettings.DefaultPaymentMethod == nil ||
		customer.InvoiceSettings.DefaultPaymentMethod.Card == nil {
		return SavedCard{}, ErrPaymentMethodNotFound
	}
	card := customer.InvoiceSettings.DefaultPaymentMethod.Card
	return SavedCard{
		Brand:    string(card.Brand),
		Last4:    card.Last4,
		ExpMonth: card.ExpMonth,
		ExpYear:  card.ExpYear,
	}, nil
}

func (g *StripeGateway) ListAttachedCards(ctx context.Context, customerID string) ([]SavedCard, error) {
	params := &stripe.PaymentMethodListParams{
		Customer: stripe.String(customerID),
		Type:     stripe.String(string(stripe.PaymentMethodTypeCard)),
	}
	cards := make([]SavedCard, 0)
	for paymentMethod, err := range g.client.V1PaymentMethods.List(ctx, params).All(ctx) {
		if err != nil {
			return nil, err
		}
		if paymentMethod.Card == nil {
			continue
		}
		cards = append(cards, SavedCard{
			Brand:    string(paymentMethod.Card.Brand),
			Last4:    paymentMethod.Card.Last4,
			ExpMonth: paymentMethod.Card.ExpMonth,
			ExpYear:  paymentMethod.Card.ExpYear,
		})
	}
	return cards, nil
}

func (g *StripeGateway) GetDefaultCard(ctx context.Context, customerID string) (SavedCard, error) {
	customer, err := g.client.V1Customers.Retrieve(ctx, customerID, defaultCardRetrieveParams())
	if err != nil {
		return SavedCard{}, err
	}
	return savedCardFromCustomer(customer)
}

func currentSetupGeneration(metadata map[string]string, generation string) bool {
	return generation != "" && metadata[MetadataSetupGeneration] == generation
}

func defaultPaymentMethodUpdateParams(paymentMethodID string) *stripe.CustomerUpdateParams {
	params := &stripe.CustomerUpdateParams{
		InvoiceSettings: &stripe.CustomerUpdateInvoiceSettingsParams{
			DefaultPaymentMethod: stripe.String(paymentMethodID),
		},
	}
	params.AddMetadata(MetadataSetupGeneration, "")
	return params
}

func (g *StripeGateway) SetDefaultPaymentMethodForSetupGeneration(ctx context.Context, customerID, paymentMethodID, generation string) (bool, error) {
	customer, err := g.client.V1Customers.Retrieve(ctx, customerID, nil)
	if err != nil {
		return false, err
	}
	if !currentSetupGeneration(customer.Metadata, generation) {
		return false, nil
	}
	_, err = g.client.V1Customers.Update(ctx, customerID, defaultPaymentMethodUpdateParams(paymentMethodID))
	return err == nil, err
}

// Checkout setup mode creates a reusable off-session SetupIntent by default; stripe-go v85 exposes no usage override here.
func setupSessionParams(request SetupSessionRequest) *stripe.CheckoutSessionCreateParams {
	setupIntentData := &stripe.CheckoutSessionCreateSetupIntentDataParams{}
	setupIntentData.AddMetadata("purpose", SetupPurpose)
	setupIntentData.AddMetadata(MetadataSetupGeneration, request.SetupGeneration)
	setupIntentData.AddMetadata(MetadataSubject, request.Subject)
	setupIntentData.AddMetadata(MetadataSetupSource, request.Source)
	return &stripe.CheckoutSessionCreateParams{
		Mode:               stripe.String(string(stripe.CheckoutSessionModeSetup)),
		Customer:           stripe.String(request.CustomerID),
		SuccessURL:         stripe.String(request.SuccessURL),
		CancelURL:          stripe.String(request.CancelURL),
		PaymentMethodTypes: []*string{stripe.String("card")},
		SetupIntentData:    setupIntentData,
	}
}

func (g *StripeGateway) CreateSetupSession(ctx context.Context, request SetupSessionRequest) (string, error) {
	session, err := g.client.V1CheckoutSessions.Create(ctx, setupSessionParams(request))
	if err != nil {
		return "", err
	}
	return session.URL, nil
}

func (g *StripeGateway) CreatePortalSession(ctx context.Context, request PortalSessionRequest) (string, error) {
	session, err := g.client.V1BillingPortalSessions.Create(ctx, &stripe.BillingPortalSessionCreateParams{
		Customer:  stripe.String(request.CustomerID),
		ReturnURL: stripe.String(request.ReturnURL),
	})
	if err != nil {
		return "", err
	}
	return session.URL, nil
}

func stripeTime(timestamp int64) time.Time {
	if timestamp == 0 {
		return time.Time{}
	}
	return time.Unix(timestamp, 0).UTC()
}

func invoiceFromStripe(source *stripe.Invoice) Invoice {
	invoice := Invoice{
		ID:          source.ID,
		Currency:    string(source.Currency),
		Total:       source.Total,
		Status:      string(source.Status),
		PeriodStart: stripeTime(source.PeriodStart),
		PeriodEnd:   stripeTime(source.PeriodEnd),
		Created:     stripeTime(source.Created),
		Lines:       []InvoiceLine{},
	}
	if source.Lines == nil {
		return invoice
	}
	for _, sourceLine := range source.Lines.Data {
		if sourceLine == nil {
			continue
		}
		line := InvoiceLine{
			Description: sourceLine.Description,
			Amount:      sourceLine.Amount,
			Quantity:    sourceLine.Quantity,
		}
		if sourceLine.Period != nil {
			line.PeriodStart = stripeTime(sourceLine.Period.Start)
			line.PeriodEnd = stripeTime(sourceLine.Period.End)
		}
		invoice.Lines = append(invoice.Lines, line)
	}
	return invoice
}

func (g *StripeGateway) PreviewInvoice(ctx context.Context, customerID string) (*Invoice, error) {
	invoice, err := g.client.V1Invoices.CreatePreview(ctx, &stripe.InvoiceCreatePreviewParams{
		Customer: stripe.String(customerID),
	})
	if err != nil {
		var stripeErr *stripe.Error
		if errors.As(err, &stripeErr) && stripeErr.Code == stripe.ErrorCodeInvoiceUpcomingNone {
			return nil, nil
		}
		return nil, err
	}
	normalized := invoiceFromStripe(invoice)
	return &normalized, nil
}

func (g *StripeGateway) ListInvoices(ctx context.Context, customerID string, createdAfter time.Time) ([]Invoice, error) {
	params := &stripe.InvoiceListParams{
		ListParams: stripe.ListParams{Limit: stripe.Int64(100)},
		Customer:   stripe.String(customerID),
		CreatedRange: &stripe.RangeQueryParams{
			GreaterThanOrEqual: createdAfter.Unix(),
		},
	}
	invoices := make([]Invoice, 0)
	for invoice, err := range g.client.V1Invoices.List(ctx, params).All(ctx) {
		if err != nil {
			return nil, err
		}
		if invoice == nil {
			continue
		}
		invoices = append(invoices, invoiceFromStripe(invoice))
	}
	return invoices, nil
}
