package billing

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"sort"
	"strings"
	"time"
)

const (
	MetadataSubject           = "fleet_subject"
	LegacyMetadataSubject     = "cyclops_subject"
	CustomerIdempotencyPrefix = "fleet-customer-"
	SetupPurpose              = "fleet_default_card"
	MetadataSetupGeneration   = "fleet_setup_generation"
	MetadataSetupSource       = "fleet_source"
)

var (
	ErrCustomerNotFound      = errors.New("billing customer not found")
	ErrPaymentMethodNotFound = errors.New("default payment method not found")
)

type Customer struct {
	ID       string
	Metadata map[string]string
}

type SavedCard struct {
	Brand    string
	Last4    string
	ExpMonth int64
	ExpYear  int64
}

type SetupSessionRequest struct {
	CustomerID      string
	SuccessURL      string
	CancelURL       string
	SetupGeneration string
	Subject         string
	Source          string
}

type PortalSessionRequest struct {
	CustomerID string
	ReturnURL  string
}

type Gateway interface {
	SearchCustomers(ctx context.Context, subject string) ([]Customer, error)
	CreateCustomer(ctx context.Context, metadata map[string]string, idempotencyKey string) (Customer, error)
	UpdateCustomerMetadata(ctx context.Context, customerID string, metadata map[string]string) (Customer, error)
	GetDefaultCard(ctx context.Context, customerID string) (SavedCard, error)
	ListAttachedCards(ctx context.Context, customerID string) ([]SavedCard, error)
	SetDefaultPaymentMethodForSetupGeneration(ctx context.Context, customerID, paymentMethodID, generation string) (bool, error)
	CreateSetupSession(ctx context.Context, request SetupSessionRequest) (string, error)
	CreatePortalSession(ctx context.Context, request PortalSessionRequest) (string, error)
	PreviewInvoice(ctx context.Context, customerID string) (*Invoice, error)
	ListInvoices(ctx context.Context, customerID string, createdAfter time.Time) ([]Invoice, error)
}

type InvoiceLine struct {
	Description string
	Amount      int64
	Quantity    int64
	PeriodStart time.Time
	PeriodEnd   time.Time
}

type Invoice struct {
	ID          string
	Currency    string
	Total       int64
	Status      string
	PeriodStart time.Time
	PeriodEnd   time.Time
	Created     time.Time
	Lines       []InvoiceLine
}

type UsagePoint struct {
	PeriodStart time.Time `json:"period_start"`
	PeriodEnd   time.Time `json:"period_end"`
	Amount      int64     `json:"amount"`
	Estimate    bool      `json:"estimate"`
}

type UsageBreakdownItem struct {
	Name        string    `json:"name"`
	Amount      int64     `json:"amount"`
	Quantity    int64     `json:"quantity"`
	PeriodStart time.Time `json:"period_start"`
	PeriodEnd   time.Time `json:"period_end"`
}

type Usage struct {
	Currency             string               `json:"currency"`
	RangeStart           time.Time            `json:"range_start"`
	RangeEnd             time.Time            `json:"range_end"`
	CurrentPeriodStart   *time.Time           `json:"current_period_start"`
	CurrentPeriodEnd     *time.Time           `json:"current_period_end"`
	CurrentEstimate      int64                `json:"current_estimate"`
	PreviousPeriodAmount int64                `json:"previous_period_amount"`
	Trend                []UsagePoint         `json:"trend"`
	Breakdown            []UsageBreakdownItem `json:"breakdown"`
}

type Service struct {
	gateway Gateway
}

func NewService(gateway Gateway) *Service {
	return &Service{gateway: gateway}
}

func (s *Service) FindCustomer(ctx context.Context, subject string) (Customer, error) {
	customers, err := s.gateway.SearchCustomers(ctx, subject)
	if err != nil {
		return Customer{}, err
	}
	for _, customer := range customers {
		if customer.Metadata[MetadataSubject] == subject {
			return customer, nil
		}
	}
	for _, customer := range customers {
		if customer.Metadata[LegacyMetadataSubject] != subject || customer.Metadata[MetadataSubject] != "" {
			continue
		}
		metadata := map[string]string{MetadataSubject: subject}
		updated, err := s.gateway.UpdateCustomerMetadata(ctx, customer.ID, metadata)
		if err != nil {
			return Customer{}, err
		}
		return updated, nil
	}
	return Customer{}, ErrCustomerNotFound
}

func (s *Service) findCustomerReadOnly(ctx context.Context, subject string) (Customer, error) {
	customers, err := s.gateway.SearchCustomers(ctx, subject)
	if err != nil {
		return Customer{}, err
	}
	for _, customer := range customers {
		if customer.Metadata[MetadataSubject] == subject {
			return customer, nil
		}
	}
	for _, customer := range customers {
		if customer.Metadata[LegacyMetadataSubject] == subject && customer.Metadata[MetadataSubject] == "" {
			return customer, nil
		}
	}
	return Customer{}, ErrCustomerNotFound
}

func (s *Service) FindOrCreateCustomer(ctx context.Context, subject string) (Customer, error) {
	customer, err := s.FindCustomer(ctx, subject)
	if err == nil {
		return customer, nil
	}
	if !errors.Is(err, ErrCustomerNotFound) {
		return Customer{}, err
	}
	metadata := map[string]string{MetadataSubject: subject}
	digest := sha256.Sum256([]byte(subject))
	created, createErr := s.gateway.CreateCustomer(ctx, metadata, CustomerIdempotencyPrefix+hex.EncodeToString(digest[:]))
	if createErr != nil {
		return created, errors.Join(createErr, err)
	}
	return created, nil
}

type CardSummary struct {
	Brand    string `json:"brand"`
	Last4    string `json:"last4"`
	ExpMonth int64  `json:"exp_month"`
	ExpYear  int64  `json:"exp_year"`
}

type Summary struct {
	PaymentMethodPresent bool         `json:"payment_method_present" binding:"required"`
	Card                 *CardSummary `json:"card" binding:"required" extensions:"x-nullable"`
	// PoolCreateCardRequired is advisory admission state the API handler
	// fills in (Service.Summary always leaves it false): true when creating
	// a pool or any other custom resource would be denied because the
	// account has no qualifying payment card. The dashboard reads it to gate
	// create flows before a request ever reaches the enforcing policy.
	PoolCreateCardRequired bool `json:"pool_create_card_required" binding:"required"`
}

func (s *Service) AttachedCards(ctx context.Context, subject string) ([]SavedCard, error) {
	customer, err := s.findCustomerReadOnly(ctx, subject)
	if errors.Is(err, ErrCustomerNotFound) {
		return []SavedCard{}, nil
	}
	if err != nil {
		return nil, err
	}
	cards, err := s.gateway.ListAttachedCards(ctx, customer.ID)
	if err != nil {
		return nil, err
	}
	if cards == nil {
		return []SavedCard{}, nil
	}
	return cards, nil
}

func (s *Service) Summary(ctx context.Context, subject string) (Summary, error) {
	customer, err := s.FindCustomer(ctx, subject)
	if errors.Is(err, ErrCustomerNotFound) {
		return Summary{}, nil
	}
	if err != nil {
		return Summary{}, err
	}

	card, err := s.gateway.GetDefaultCard(ctx, customer.ID)
	if errors.Is(err, ErrPaymentMethodNotFound) {
		return Summary{}, nil
	}
	if err != nil {
		return Summary{}, err
	}

	return Summary{
		PaymentMethodPresent: true,
		Card: &CardSummary{
			Brand:    card.Brand,
			Last4:    card.Last4,
			ExpMonth: card.ExpMonth,
			ExpYear:  card.ExpYear,
		},
	}, nil
}

func (s *Service) Usage(ctx context.Context, subject string, months int, now time.Time) (Usage, error) {
	end := now.UTC()
	start := end.AddDate(0, -months, 0)
	empty := Usage{
		Currency:   "usd",
		RangeStart: start,
		RangeEnd:   end,
		Trend:      []UsagePoint{},
		Breakdown:  []UsageBreakdownItem{},
	}

	customer, err := s.findCustomerReadOnly(ctx, subject)
	if errors.Is(err, ErrCustomerNotFound) {
		return empty, nil
	}
	if err != nil {
		return Usage{}, err
	}

	invoices, err := s.gateway.ListInvoices(ctx, customer.ID, start)
	if err != nil {
		return Usage{}, err
	}
	preview, err := s.gateway.PreviewInvoice(ctx, customer.ID)
	if err != nil {
		return Usage{}, err
	}

	currency := ""
	if preview != nil {
		currency = preview.Currency
	}
	if currency == "" && len(invoices) > 0 {
		currency = invoices[0].Currency
	}
	if currency == "" {
		currency = empty.Currency
	}

	usage := empty
	usage.Currency = strings.ToLower(currency)
	for _, invoice := range invoices {
		if !strings.EqualFold(invoice.Currency, usage.Currency) || invoice.Status == "draft" || invoice.Status == "void" {
			continue
		}
		usage.Trend = append(usage.Trend, UsagePoint{
			PeriodStart: invoice.PeriodStart,
			PeriodEnd:   invoice.PeriodEnd,
			Amount:      invoice.Total,
		})
	}
	sort.Slice(usage.Trend, func(i, j int) bool {
		return usage.Trend[i].PeriodStart.Before(usage.Trend[j].PeriodStart)
	})
	if len(usage.Trend) > 0 {
		usage.PreviousPeriodAmount = usage.Trend[len(usage.Trend)-1].Amount
	}

	if preview == nil || !strings.EqualFold(preview.Currency, usage.Currency) {
		return usage, nil
	}
	usage.CurrentPeriodStart = &preview.PeriodStart
	usage.CurrentPeriodEnd = &preview.PeriodEnd
	usage.CurrentEstimate = preview.Total
	usage.Trend = append(usage.Trend, UsagePoint{
		PeriodStart: preview.PeriodStart,
		PeriodEnd:   preview.PeriodEnd,
		Amount:      preview.Total,
		Estimate:    true,
	})

	byName := map[string]UsageBreakdownItem{}
	for _, line := range preview.Lines {
		name := strings.TrimSpace(line.Description)
		if name == "" {
			name = "Usage"
		}
		item := byName[name]
		item.Name = name
		item.Amount += line.Amount
		item.Quantity += line.Quantity
		if item.PeriodStart.IsZero() || line.PeriodStart.Before(item.PeriodStart) {
			item.PeriodStart = line.PeriodStart
		}
		if line.PeriodEnd.After(item.PeriodEnd) {
			item.PeriodEnd = line.PeriodEnd
		}
		byName[name] = item
	}
	for _, item := range byName {
		usage.Breakdown = append(usage.Breakdown, item)
	}
	sort.Slice(usage.Breakdown, func(i, j int) bool {
		left := usage.Breakdown[i].Amount
		if left < 0 {
			left = -left
		}
		right := usage.Breakdown[j].Amount
		if right < 0 {
			right = -right
		}
		if left == right {
			return usage.Breakdown[i].Name < usage.Breakdown[j].Name
		}
		return left > right
	})
	return usage, nil
}

type SetupOptions struct {
	SuccessURL string
	CancelURL  string
	Source     string
}

func (s *Service) CreateSetupSession(ctx context.Context, subject string, options SetupOptions) (string, error) {
	customer, err := s.FindOrCreateCustomer(ctx, subject)
	if err != nil {
		return "", err
	}
	generation, err := newSetupGeneration()
	if err != nil {
		return "", err
	}
	url, err := s.gateway.CreateSetupSession(ctx, SetupSessionRequest{
		CustomerID:      customer.ID,
		SuccessURL:      options.SuccessURL,
		CancelURL:       options.CancelURL,
		SetupGeneration: generation,
		Subject:         subject,
		Source:          options.Source,
	})
	if err != nil {
		return "", err
	}
	if _, err := s.gateway.UpdateCustomerMetadata(ctx, customer.ID, map[string]string{MetadataSetupGeneration: generation}); err != nil {
		return "", err
	}
	return url, nil
}

func (s *Service) CreatePortalSession(ctx context.Context, subject, returnURL string) (string, error) {
	customer, err := s.FindCustomer(ctx, subject)
	if err != nil {
		return "", err
	}
	return s.gateway.CreatePortalSession(ctx, PortalSessionRequest{
		CustomerID: customer.ID,
		ReturnURL:  returnURL,
	})
}

func newSetupGeneration() (string, error) {
	bytes := make([]byte, 32)
	if _, err := rand.Read(bytes); err != nil {
		return "", err
	}
	return hex.EncodeToString(bytes), nil
}

func (s *Service) SetDefaultPaymentMethodForSetupGeneration(ctx context.Context, customerID, paymentMethodID, generation string) (bool, error) {
	if customerID == "" || paymentMethodID == "" || generation == "" {
		return false, errors.New("customer, payment method, and setup generation are required")
	}
	return s.gateway.SetDefaultPaymentMethodForSetupGeneration(ctx, customerID, paymentMethodID, generation)
}
