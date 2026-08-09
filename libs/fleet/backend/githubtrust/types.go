package githubtrust

import (
	"errors"
	"regexp"
	"slices"
	"strings"
	"time"
)

var (
	repositoryPattern = regexp.MustCompile(`^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$`)
	namespacePattern  = regexp.MustCompile(`^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`)

	ErrInvalidRepository = errors.New("repository must use owner/repo format")
	ErrEmptyNamespaces   = errors.New("allowed_namespaces must not be empty")
	ErrInvalidNamespace  = errors.New("allowed_namespaces must be DNS-1123 labels")
)

// Policy is the runtime representation of a GitHub Actions OIDC trust policy.
// GORM tags drive AutoMigrate so the schema is derived from this struct —
// no manual DDL. pgx is still used for queries; GORM is migration-only.
type Policy struct {
	ID                string    `json:"id" gorm:"primaryKey"`
	OwnerSub          string    `json:"owner_sub" gorm:"column:owner_sub;index"`
	Name              string    `json:"name"`
	Repository        string    `json:"repository" gorm:"column:repository;index"`
	AllowedNamespaces []string  `json:"allowed_namespaces" gorm:"type:text[]"`
	Enabled           bool      `json:"enabled"`
	CreatedAt         time.Time `json:"created_at"`
	UpdatedAt         time.Time `json:"updated_at"`
}

// TableName overrides GORM's default pluralisation so the table name matches
// what the pgx queries expect.
func (Policy) TableName() string { return "github_trust_policies" }

type PolicyInput struct {
	Name              string   `json:"name"`
	Repository        string   `json:"repository"`
	AllowedNamespaces []string `json:"allowed_namespaces"`
	Enabled           bool     `json:"enabled"`
}

func NormalizePolicyInput(input PolicyInput) (*Policy, error) {
	repository := strings.TrimSpace(input.Repository)
	if !repositoryPattern.MatchString(repository) {
		return nil, ErrInvalidRepository
	}
	namespaces := normalizeNamespaces(input.AllowedNamespaces)
	if len(namespaces) == 0 {
		return nil, ErrEmptyNamespaces
	}
	for _, ns := range namespaces {
		if !namespacePattern.MatchString(ns) || len(ns) > 63 {
			return nil, ErrInvalidNamespace
		}
	}
	return &Policy{
		Name:              strings.TrimSpace(input.Name),
		Repository:        repository,
		AllowedNamespaces: namespaces,
		Enabled:           input.Enabled,
	}, nil
}

func normalizeNamespaces(in []string) []string {
	seen := map[string]struct{}{}
	out := make([]string, 0, len(in))
	for _, raw := range in {
		ns := strings.TrimSpace(raw)
		if ns == "" {
			continue
		}
		if _, ok := seen[ns]; ok {
			continue
		}
		seen[ns] = struct{}{}
		out = append(out, ns)
	}
	slices.Sort(out)
	return out
}

func sortByUpdatedDesc(policies []*Policy) {
	slices.SortFunc(policies, func(a, b *Policy) int {
		switch {
		case a.UpdatedAt.After(b.UpdatedAt):
			return -1
		case a.UpdatedAt.Before(b.UpdatedAt):
			return 1
		default:
			return strings.Compare(a.ID, b.ID)
		}
	})
}
