package signedurls

import (
	"context"
	"errors"
	"time"

	"github.com/google/uuid"
)

const (
	minTTL      = time.Minute
	maxTTL      = 24 * time.Hour
	maxLabelLen = 120
)

var (
	ErrNotFound          = errors.New("signed service URL not found")
	ErrInvalidCapability = errors.New("invalid signed service capability")
	ErrInvalidInput      = errors.New("invalid signed service URL input")
	ErrUnavailable       = errors.New("signed service URLs unavailable")
)

type Record struct {
	ID             uuid.UUID
	Namespace      string
	ClaimName      string
	SandboxName    string
	ServiceName    string
	LogicalService string
	Label          *string
	CreatorSub     string
	CreatedAt      time.Time
	ExpiresAt      time.Time
	RevokedAt      *time.Time
	URL            string
}

type CreateInput struct {
	Namespace      string
	ClaimName      string
	SandboxName    string
	ServiceName    string
	LogicalService string
	Label          *string
	CreatorSub     string
	ExpiresIn      time.Duration
}

type Store interface {
	Create(context.Context, Record) error
	List(context.Context, string, string) ([]Record, error)
	Get(context.Context, uuid.UUID) (Record, error)
	Revoke(context.Context, string, uuid.UUID, time.Time) (Record, error)
}
