package accountlookup

import (
	"context"
	"errors"

	"cyclops-cs-backend/internal/redactederror"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

type Store struct{ pool *pgxpool.Pool }

// NewStore is deliberately lazy: database availability must not gate startup.
func NewStore(ctx context.Context, dsn string) (*Store, error) {
	if dsn == "" {
		return nil, errors.New("account lookup database is not configured")
	}
	config, err := pgxpool.ParseConfig(dsn)
	if err != nil {
		return nil, redactederror.New("invalid account lookup database configuration", err)
	}
	config.MaxConns = 4
	pool, err := pgxpool.NewWithConfig(ctx, config)
	if err != nil {
		return nil, redactederror.New("invalid account lookup database configuration", err)
	}
	return &Store{pool: pool}, nil
}

func (s *Store) Close() { s.pool.Close() }

func (s *Store) Record(ctx context.Context, realm, keyID, pseudonym, subject string) error {
	tag, err := s.pool.Exec(ctx, `INSERT INTO account_lookup_private.mapping (realm,key_id,pseudonym,subject)
 VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING`, realm, keyID, pseudonym, subject)
	if err != nil {
		return redactederror.New("account lookup record unavailable", err)
	}
	if tag.RowsAffected() == 0 {
		existing, found, err := s.Resolve(ctx, realm, keyID, pseudonym)
		if err != nil {
			return err
		}
		if !found || existing != subject {
			return errors.New("account lookup mapping conflict")
		}
	}
	return nil
}

func (s *Store) Resolve(ctx context.Context, realm, keyID, pseudonym string) (string, bool, error) {
	var subject string
	err := s.pool.QueryRow(ctx, `SELECT subject FROM account_lookup_private.mapping WHERE realm=$1 AND key_id=$2 AND pseudonym=$3`, realm, keyID, pseudonym).Scan(&subject)
	if errors.Is(err, pgx.ErrNoRows) {
		return "", false, nil
	}
	if err != nil {
		return "", false, redactederror.New("account lookup unavailable", err)
	}
	return subject, true, nil
}

func (s *Store) Complete(ctx context.Context, realm, keyID string) (bool, error) {
	var complete bool
	err := s.pool.QueryRow(ctx, `SELECT EXISTS(SELECT 1 FROM account_lookup_private.backfill WHERE realm=$1 AND key_id=$2)`, realm, keyID).Scan(&complete)
	if err != nil {
		return false, redactederror.New("account lookup scan status unavailable", err)
	}
	return complete, nil
}

// MarkScanComplete is called only after an uninterrupted scan starting at zero.
func (s *Store) MarkScanComplete(ctx context.Context, realm, keyID string) error {
	_, err := s.pool.Exec(ctx, `INSERT INTO account_lookup_private.backfill (realm,key_id) VALUES ($1,$2)
 ON CONFLICT (realm,key_id) DO UPDATE SET observed_scan_completed_at=clock_timestamp()`, realm, keyID)
	if err != nil {
		return redactederror.New("account lookup scan status unavailable", err)
	}
	return nil
}

func (s *Store) Audit(ctx context.Context, actor, outcome string) error {
	_, err := s.pool.Exec(ctx, `INSERT INTO account_lookup_private.audit (actor,outcome) VALUES ($1,$2)`, actor, outcome)
	if err != nil {
		return redactederror.New("account lookup audit unavailable", err)
	}
	return nil
}

// Allow serializes each actor across replicas and enforces a rolling minute.
func (s *Store) Allow(ctx context.Context, actor string) (bool, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return false, redactederror.New("account lookup rate limit unavailable", err)
	}
	defer tx.Rollback(ctx)
	if _, err = tx.Exec(ctx, `INSERT INTO account_lookup_private.rate_limit (actor) VALUES ($1) ON CONFLICT DO NOTHING`, actor); err != nil {
		return false, redactederror.New("account lookup rate limit unavailable", err)
	}
	var locked string
	if err = tx.QueryRow(ctx, `SELECT actor FROM account_lookup_private.rate_limit WHERE actor=$1 FOR UPDATE`, actor).Scan(&locked); err != nil {
		return false, redactederror.New("account lookup rate limit unavailable", err)
	}
	var allowed bool
	err = tx.QueryRow(ctx, `WITH recent AS (
 SELECT ARRAY(SELECT t FROM unnest(attempts) t WHERE t > clock_timestamp()-interval '1 minute') AS times
 FROM account_lookup_private.rate_limit WHERE actor=$1
 ) UPDATE account_lookup_private.rate_limit SET attempts=recent.times || clock_timestamp()
 FROM recent WHERE actor=$1 AND cardinality(recent.times)<10 RETURNING true`, actor).Scan(&allowed)
	if errors.Is(err, pgx.ErrNoRows) {
		err = nil
	}
	if err != nil {
		return false, redactederror.New("account lookup rate limit unavailable", err)
	}
	if err = tx.Commit(ctx); err != nil {
		return false, redactederror.New("account lookup rate limit unavailable", err)
	}
	return allowed, nil
}
