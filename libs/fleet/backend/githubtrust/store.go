package githubtrust

// Postgres-backed persistence for GitHub Actions OIDC trust policies.
//
// Listing is indexed by owner_sub (List); ResolveByRepository (the per-request
// auth hot path) is indexed by repository. Every read takes the owner so the
// storage layer enforces the same tenant isolation the HTTP layer does —
// CUA-675 moved this off Redis so policies survive a Pod eviction (the
// cyclops-cs Redis persists to an emptyDir, which does not).

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// ErrNotFound is returned by Update when the policy no longer exists for the
// given owner. Handlers Get-then-Update, so this is a race backstop.
var ErrNotFound = errors.New("github trust policy not found")

const policyColumns = "id, owner_sub, name, repository, allowed_namespaces, enabled, created_at, updated_at"

type Store interface {
	List(ctx context.Context, ownerSub string) ([]*Policy, error)
	Create(ctx context.Context, policy *Policy) error
	Get(ctx context.Context, ownerSub, id string) (*Policy, error)
	Update(ctx context.Context, policy *Policy) error
	Delete(ctx context.Context, ownerSub, id string) (bool, error)
	ResolveByRepository(ctx context.Context, repository string) ([]*Policy, error)
}

type pgStore struct{ pool *pgxpool.Pool }

// New opens and verifies a connection pool against url (a Postgres DSN or URL).
// An empty url disables the feature (returns nil, nil) so the handlers reply
// 503, mirroring the previous Redis behaviour. A startup connectivity blip is
// non-fatal to the process: initialization returns an error so the caller can
// leave the routes disabled rather than exposing a store without its table.
func New(ctx context.Context, url string) (Store, error) {
	if url == "" {
		return nil, nil
	}

	pool, err := pgxpool.New(ctx, url)
	if err != nil {
		return nil, fmt.Errorf("connect postgres: %w", err)
	}
	initCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()

	if err := pool.Ping(initCtx); err != nil {
		pool.Close()
		return nil, fmt.Errorf("ping postgres: %w", err)
	}

	return &pgStore{pool: pool}, nil
}

func (s *pgStore) List(ctx context.Context, ownerSub string) ([]*Policy, error) {
	rows, err := s.pool.Query(ctx,
		`SELECT `+policyColumns+` FROM github_trust_policies
		 WHERE owner_sub = $1
		 ORDER BY updated_at DESC, id ASC`, ownerSub)
	if err != nil {
		return nil, fmt.Errorf("list policies: %w", err)
	}
	return collect(rows)
}

func (s *pgStore) Create(ctx context.Context, policy *Policy) error {
	now := time.Now().UTC()
	if policy.ID == "" {
		policy.ID = uuid.NewString()
	}
	if policy.CreatedAt.IsZero() {
		policy.CreatedAt = now
	}
	policy.UpdatedAt = now
	_, err := s.pool.Exec(ctx,
		`INSERT INTO github_trust_policies (`+policyColumns+`)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)`,
		policy.ID, policy.OwnerSub, policy.Name, policy.Repository,
		policy.AllowedNamespaces, policy.Enabled, policy.CreatedAt, policy.UpdatedAt)
	if err != nil {
		return fmt.Errorf("create policy: %w", err)
	}
	return nil
}

func (s *pgStore) Get(ctx context.Context, ownerSub, id string) (*Policy, error) {
	row := s.pool.QueryRow(ctx,
		`SELECT `+policyColumns+` FROM github_trust_policies
		 WHERE id = $1 AND owner_sub = $2`, id, ownerSub)
	policy, err := scanPolicy(row)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("get policy: %w", err)
	}
	return policy, nil
}

func (s *pgStore) Update(ctx context.Context, policy *Policy) error {
	policy.UpdatedAt = time.Now().UTC()
	tag, err := s.pool.Exec(ctx,
		`UPDATE github_trust_policies
		 SET name = $3, repository = $4, allowed_namespaces = $5, enabled = $6, updated_at = $7
		 WHERE id = $1 AND owner_sub = $2`,
		policy.ID, policy.OwnerSub, policy.Name, policy.Repository,
		policy.AllowedNamespaces, policy.Enabled, policy.UpdatedAt)
	if err != nil {
		return fmt.Errorf("update policy: %w", err)
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

func (s *pgStore) Delete(ctx context.Context, ownerSub, id string) (bool, error) {
	tag, err := s.pool.Exec(ctx,
		`DELETE FROM github_trust_policies WHERE id = $1 AND owner_sub = $2`, id, ownerSub)
	if err != nil {
		return false, fmt.Errorf("delete policy: %w", err)
	}
	return tag.RowsAffected() > 0, nil
}

func (s *pgStore) ResolveByRepository(ctx context.Context, repository string) ([]*Policy, error) {
	rows, err := s.pool.Query(ctx,
		`SELECT `+policyColumns+` FROM github_trust_policies
		 WHERE repository = $1
		 ORDER BY updated_at DESC, id ASC`, repository)
	if err != nil {
		return nil, fmt.Errorf("resolve policies by repository: %w", err)
	}
	return collect(rows)
}

// collect scans all rows from a pgx.Rows into a slice of policies.
func collect(rows pgx.Rows) ([]*Policy, error) {
	out := make([]*Policy, 0)
	for rows.Next() {
		policy, err := scanPolicy(rows)
		if err != nil {
			rows.Close()
			return nil, err
		}
		out = append(out, policy)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("scan policies: %w", err)
	}
	return out, nil
}

// scanner is implemented by both pgx.Row and pgx.Rows.
type scanner interface {
	Scan(dest ...any) error
}

func scanPolicy(r scanner) (*Policy, error) {
	p := &Policy{}
	err := r.Scan(
		&p.ID, &p.OwnerSub, &p.Name, &p.Repository,
		&p.AllowedNamespaces, &p.Enabled, &p.CreatedAt, &p.UpdatedAt,
	)
	if err != nil {
		return nil, err
	}
	return p, nil
}
