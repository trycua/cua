package billing

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"errors"
)

const (
	MetadataSubject           = "fleet_subject"
	LegacyMetadataSubject     = "cyclops_subject"
	CustomerIdempotencyPrefix = "fleet-customer-"
	SetupPurpose              = "fleet_default_card"
	MetadataSetupGeneration   = "fleet_setup_generation"
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
	SetDefaultPaymentMethodForSetupGeneration(ctx context.Context, customerID, paymentMethodID, generation string) (bool, error)
	CreateSetupSession(ctx context.Context, request SetupSessionRequest) (string, error)
	CreatePortalSession(ctx context.Context, request PortalSessionRequest) (string, error)
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
	return s.gateway.CreateCustomer(ctx, metadata, CustomerIdempotencyPrefix+hex.EncodeToString(digest[:]))
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

type SetupOptions struct {
	SuccessURL string
	CancelURL  string
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
