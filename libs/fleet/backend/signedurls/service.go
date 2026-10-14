package signedurls

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/google/uuid"
)

type Service struct {
	store  Store
	signer *Signer
	now    func() time.Time
	newID  func() (uuid.UUID, error)
}

func NewService(store Store, signer *Signer) *Service {
	return &Service{store: store, signer: signer, now: func() time.Time { return time.Now().UTC() }, newID: uuid.NewRandom}
}

func (service *Service) Create(ctx context.Context, input CreateInput) (Record, error) {
	if err := service.available(); err != nil {
		return Record{}, err
	}
	input, err := normalizeInput(input)
	if err != nil {
		return Record{}, err
	}
	id, err := service.newID()
	if err != nil {
		return Record{}, fmt.Errorf("generate signed service URL ID: %w", err)
	}
	createdAt := service.now().UTC().Truncate(time.Second)
	record := Record{ID: id, Namespace: input.Namespace, ClaimName: input.ClaimName, SandboxName: input.SandboxName, ServiceName: input.ServiceName, LogicalService: input.LogicalService, Label: input.Label, CreatorSub: input.CreatorSub, CreatedAt: createdAt, ExpiresAt: createdAt.Add(input.ExpiresIn)}
	if err := service.store.Create(ctx, record); err != nil {
		return Record{}, fmt.Errorf("store signed service URL: %w", err)
	}
	return service.withURL(record)
}

func (service *Service) List(ctx context.Context, namespace, claimName string) ([]Record, error) {
	if err := service.available(); err != nil {
		return nil, err
	}
	records, err := service.store.List(ctx, namespace, claimName)
	if err != nil {
		return nil, fmt.Errorf("list signed service URLs: %w", err)
	}
	for index := range records {
		if records[index], err = service.withURL(records[index]); err != nil {
			return nil, err
		}
	}
	return records, nil
}

func (service *Service) Revoke(ctx context.Context, namespace string, id uuid.UUID) (Record, error) {
	if err := service.available(); err != nil {
		return Record{}, err
	}
	record, err := service.store.Revoke(ctx, namespace, id, service.now().UTC())
	if err != nil {
		return Record{}, err
	}
	return service.withURL(record)
}

func (service *Service) Resolve(ctx context.Context, token string, now time.Time) (Record, error) {
	if err := service.available(); err != nil {
		return Record{}, err
	}
	capability, err := service.signer.Verify(token, now)
	if err != nil {
		return Record{}, err
	}
	record, err := service.store.Get(ctx, capability.ID)
	if errors.Is(err, ErrNotFound) {
		return Record{}, ErrInvalidCapability
	}
	if err != nil {
		return Record{}, fmt.Errorf("get signed service URL: %w", err)
	}
	if record.Namespace != capability.Namespace || record.ServiceName != capability.ServiceName || !record.ExpiresAt.Equal(capability.ExpiresAt) || record.RevokedAt != nil || !now.Before(record.ExpiresAt) {
		return Record{}, ErrInvalidCapability
	}
	return record, nil
}

func (service *Service) available() error {
	if service == nil || service.store == nil || service.signer == nil {
		return ErrUnavailable
	}
	return nil
}
func (service *Service) withURL(record Record) (Record, error) {
	capabilityURL, err := service.signer.URL(record)
	if err != nil {
		return Record{}, err
	}
	record.URL = capabilityURL
	return record, nil
}
func normalizeInput(input CreateInput) (CreateInput, error) {
	fields := []*string{&input.Namespace, &input.ClaimName, &input.SandboxName, &input.ServiceName, &input.LogicalService, &input.CreatorSub}
	for _, field := range fields {
		*field = strings.TrimSpace(*field)
		if *field == "" {
			return CreateInput{}, ErrInvalidInput
		}
	}
	if input.ExpiresIn < minTTL || input.ExpiresIn > maxTTL || input.ExpiresIn%time.Second != 0 {
		return CreateInput{}, ErrInvalidInput
	}
	if input.Label != nil {
		label := strings.TrimSpace(*input.Label)
		if label == "" {
			input.Label = nil
		} else if !utf8.ValidString(label) || len(label) > maxLabelLen {
			return CreateInput{}, ErrInvalidInput
		} else {
			input.Label = &label
		}
	}
	return input, nil
}
