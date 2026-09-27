package signedurls

import (
	"context"
	"errors"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
)

func TestServiceCreateNormalizesAndSignsRecord(t *testing.T) {
	store := &fakeStore{}
	service := newTestService(store)
	service.now = func() time.Time { return time.Date(2026, time.August, 31, 12, 0, 0, 123, time.UTC) }

	created, err := service.Create(context.Background(), CreateInput{
		Namespace: "tenant-a", ClaimName: "claim-a", SandboxName: "sandbox-a",
		ServiceName: "sandbox-a-mcp", LogicalService: "mcp",
		Label: ptr("  Customer demo  "), CreatorSub: "user-a", ExpiresIn: time.Hour,
	})
	if err != nil {
		t.Fatalf("Create() error = %v", err)
	}
	if created.Label == nil || *created.Label != "Customer demo" {
		t.Fatalf("Create() label = %#v, want trimmed Customer demo", created.Label)
	}
	if created.URL == "" || !strings.Contains(created.URL, "/api/signed-svc/v1.") {
		t.Fatalf("Create() URL = %q, want signed capability URL", created.URL)
	}
	if created.CreatedAt.Nanosecond() != 0 || !created.ExpiresAt.Equal(created.CreatedAt.Add(time.Hour)) {
		t.Fatalf("Create() times = %s, %s", created.CreatedAt, created.ExpiresAt)
	}
}

func TestServiceCreateValidation(t *testing.T) {
	service := newTestService(&fakeStore{})
	base := CreateInput{Namespace: "tenant-a", ClaimName: "claim-a", SandboxName: "sandbox-a", ServiceName: "sandbox-a-mcp", LogicalService: "mcp", CreatorSub: "user-a", ExpiresIn: time.Hour}
	cases := []struct {
		name  string
		input CreateInput
	}{
		{"empty identifier", func() CreateInput { input := base; input.ServiceName = "  "; return input }()},
		{"short ttl", func() CreateInput { input := base; input.ExpiresIn = 59 * time.Second; return input }()},
		{"long ttl", func() CreateInput { input := base; input.ExpiresIn = 86401 * time.Second; return input }()},
		{"long label", func() CreateInput { input := base; input.Label = ptr(strings.Repeat("a", 121)); return input }()},
	}
	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			if _, err := service.Create(context.Background(), testCase.input); err == nil {
				t.Fatal("Create() succeeded")
			}
		})
	}

	base.Label = ptr(" \t ")
	created, err := service.Create(context.Background(), base)
	if err != nil {
		t.Fatalf("Create() empty label error = %v", err)
	}
	if created.Label != nil {
		t.Fatalf("Create() empty label = %q, want nil", *created.Label)
	}
}

func TestServiceListsNewestFirstAndSignsEveryRecord(t *testing.T) {
	now := time.Date(2026, time.August, 31, 12, 0, 0, 0, time.UTC)
	older := signedURLFixture()
	older.ID = uuid.MustParse("00000000-0000-4000-8000-000000000001")
	older.ClaimName = "claim-a"
	older.CreatedAt = now.Add(-time.Minute)
	newer := older
	newer.ID = uuid.MustParse("00000000-0000-4000-8000-000000000002")
	newer.CreatedAt = now
	store := &fakeStore{records: map[uuid.UUID]Record{older.ID: older, newer.ID: newer}}
	service := newTestService(store)

	listed, err := service.List(context.Background(), "tenant-a", "claim-a")
	if err != nil {
		t.Fatalf("List() error = %v", err)
	}
	if len(listed) != 2 || listed[0].ID != newer.ID || listed[0].URL == "" || listed[1].URL == "" {
		t.Fatalf("List() = %#v", listed)
	}
}

func TestServiceRevokeIsIdempotent(t *testing.T) {
	record := signedURLFixture()
	record.ClaimName = "claim-a"
	store := &fakeStore{records: map[uuid.UUID]Record{record.ID: record}}
	service := newTestService(store)
	now := time.Date(2026, time.August, 31, 12, 15, 0, 0, time.UTC)
	service.now = func() time.Time { return now }

	first, err := service.Revoke(context.Background(), "tenant-a", record.ID)
	if err != nil || first.RevokedAt == nil || !first.RevokedAt.Equal(now) {
		t.Fatalf("first Revoke() = %#v, %v", first, err)
	}
	now = now.Add(time.Minute)
	second, err := service.Revoke(context.Background(), "tenant-a", record.ID)
	if err != nil || second.RevokedAt == nil || !second.RevokedAt.Equal(*first.RevokedAt) {
		t.Fatalf("second Revoke() = %#v, %v", second, err)
	}
}

func TestServiceResolveRejectsInvalidLifecycleAndMismatches(t *testing.T) {
	record := signedURLFixture()
	store := &fakeStore{records: map[uuid.UUID]Record{record.ID: record}}
	service := newTestService(store)
	url, err := service.signer.URL(record)
	if err != nil {
		t.Fatalf("URL() error = %v", err)
	}
	token := url[len("https://run.cua.ai/api/signed-svc/") : len(url)-1]

	resolved, err := service.Resolve(context.Background(), token, record.CreatedAt)
	if err != nil || resolved.ID != record.ID {
		t.Fatalf("Resolve() = %#v, %v", resolved, err)
	}

	for name, mutateRecord := range map[string]func(*Record){
		"expired":          func(record *Record) { record.ExpiresAt = record.CreatedAt },
		"revoked":          func(record *Record) { revokedAt := record.CreatedAt; record.RevokedAt = &revokedAt },
		"payload mismatch": func(record *Record) { record.ServiceName = "other-service" },
	} {
		t.Run(name, func(t *testing.T) {
			invalidStore := &fakeStore{records: map[uuid.UUID]Record{record.ID: record}}
			candidate := invalidStore.records[record.ID]
			mutateRecord(&candidate)
			invalidStore.records[record.ID] = candidate
			invalidService := newTestService(invalidStore)
			if _, err := invalidService.Resolve(context.Background(), token, record.CreatedAt); !errors.Is(err, ErrInvalidCapability) {
				t.Fatalf("Resolve() error = %v, want ErrInvalidCapability", err)
			}
		})
	}
}

func TestServiceUnavailableStore(t *testing.T) {
	service := newTestService(nil)
	if _, err := service.List(context.Background(), "tenant-a", "claim-a"); !errors.Is(err, ErrUnavailable) {
		t.Fatalf("List() error = %v, want ErrUnavailable", err)
	}
}

type fakeStore struct {
	records map[uuid.UUID]Record
	err     error
}

func (store *fakeStore) Create(_ context.Context, record Record) error {
	if err := store.err; err != nil {
		return err
	}
	if store.records == nil {
		store.records = make(map[uuid.UUID]Record)
	}
	store.records[record.ID] = record
	return nil
}

func (store *fakeStore) List(_ context.Context, namespace, claimName string) ([]Record, error) {
	if err := store.err; err != nil {
		return nil, err
	}
	var records []Record
	for _, record := range store.records {
		if record.Namespace == namespace && record.ClaimName == claimName {
			records = append(records, record)
		}
	}
	sort.Slice(records, func(left, right int) bool {
		if records[left].CreatedAt.Equal(records[right].CreatedAt) {
			return records[left].ID.String() < records[right].ID.String()
		}
		return records[left].CreatedAt.After(records[right].CreatedAt)
	})
	return records, nil
}

func (store *fakeStore) Get(_ context.Context, id uuid.UUID) (Record, error) {
	if err := store.err; err != nil {
		return Record{}, err
	}
	record, ok := store.records[id]
	if !ok {
		return Record{}, ErrNotFound
	}
	return record, nil
}

func (store *fakeStore) Revoke(_ context.Context, namespace string, id uuid.UUID, revokedAt time.Time) (Record, error) {
	if err := store.err; err != nil {
		return Record{}, err
	}
	record, ok := store.records[id]
	if !ok || record.Namespace != namespace {
		return Record{}, ErrNotFound
	}
	if record.RevokedAt == nil {
		record.RevokedAt = &revokedAt
		store.records[id] = record
	}
	return record, nil
}

func newTestService(store Store) *Service {
	signer, err := NewSigner("https://run.cua.ai", []byte("01234567890123456789012345678901"))
	if err != nil {
		panic(err)
	}
	return NewService(store, signer)
}

func ptr(value string) *string { return &value }

func TestServiceCreateRejectsFractionalSecondTTL(t *testing.T) {
	service := newTestService(&fakeStore{})
	_, err := service.Create(context.Background(), CreateInput{
		Namespace: "tenant-a", ClaimName: "claim-a", SandboxName: "sandbox-a",
		ServiceName: "sandbox-a-mcp", LogicalService: "mcp", CreatorSub: "user-a",
		ExpiresIn: 90*time.Second + time.Millisecond,
	})
	if !errors.Is(err, ErrInvalidInput) {
		t.Fatalf("Create() error = %v, want ErrInvalidInput", err)
	}
}

func TestServiceResolveReturnsGenericInvalidCapabilityError(t *testing.T) {
	record := signedURLFixture()
	signer := newTestService(&fakeStore{}).signer
	url, err := signer.URL(record)
	if err != nil {
		t.Fatalf("URL() error = %v", err)
	}
	validToken := url[len("https://run.cua.ai/api/signed-svc/") : len(url)-1]

	revoked := record
	revokedAt := record.CreatedAt
	revoked.RevokedAt = &revokedAt
	mismatched := record
	mismatched.ServiceName = "other-service"

	tests := []struct {
		name    string
		token   string
		now     time.Time
		records map[uuid.UUID]Record
	}{
		{name: "malformed", token: "not-a-token", now: record.CreatedAt, records: map[uuid.UUID]Record{record.ID: record}},
		{name: "expired", token: validToken, now: record.ExpiresAt, records: map[uuid.UUID]Record{record.ID: record}},
		{name: "revoked", token: validToken, now: record.CreatedAt, records: map[uuid.UUID]Record{record.ID: revoked}},
		{name: "unknown ID", token: validToken, now: record.CreatedAt, records: map[uuid.UUID]Record{}},
		{name: "payload mismatch", token: validToken, now: record.CreatedAt, records: map[uuid.UUID]Record{record.ID: mismatched}},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			service := newTestService(&fakeStore{records: test.records})
			if _, err := service.Resolve(context.Background(), test.token, test.now); !errors.Is(err, ErrInvalidCapability) {
				t.Fatalf("Resolve() error = %v, want ErrInvalidCapability", err)
			}
		})
	}
}
