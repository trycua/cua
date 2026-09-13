package signedurls

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
)

const recordColumns = "id, namespace, claim_name, sandbox_name, service_name, logical_service, label, creator_sub, created_at, expires_at, revoked_at"

type PostgresStore struct{ pool *pgxpool.Pool }

func NewPostgresStore(ctx context.Context, databaseURL string) (*PostgresStore, error) {
	config, err := pgxpool.ParseConfig(databaseURL)
	if err != nil {
		return nil, fmt.Errorf("parse signed service URL database URL: %w", err)
	}
	pool, err := pgxpool.NewWithConfig(ctx, config)
	if err != nil {
		return nil, signedServiceURLDatabaseError("connect signed service URL database", err)
	}
	pingCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	if err := pool.Ping(pingCtx); err != nil {
		pool.Close()
		return nil, signedServiceURLDatabaseError("ping signed service URL database", err)
	}
	return &PostgresStore{pool: pool}, nil
}

func (store *PostgresStore) Close() {
	if store != nil && store.pool != nil {
		store.pool.Close()
	}
}

func (store *PostgresStore) Create(ctx context.Context, record Record) error {
	_, err := store.pool.Exec(ctx, `INSERT INTO signed_service_urls (`+recordColumns+`) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)`, record.ID, record.Namespace, record.ClaimName, record.SandboxName, record.ServiceName, record.LogicalService, record.Label, record.CreatorSub, record.CreatedAt, record.ExpiresAt, record.RevokedAt)
	if err != nil {
		return signedServiceURLDatabaseError("create signed service URL", err)
	}
	return nil
}

func (store *PostgresStore) List(ctx context.Context, namespace, claimName string) ([]Record, error) {
	rows, err := store.pool.Query(ctx, `SELECT `+recordColumns+` FROM signed_service_urls WHERE namespace = $1 AND claim_name = $2 ORDER BY created_at DESC, id`, namespace, claimName)
	if err != nil {
		return nil, signedServiceURLDatabaseError("list signed service URLs", err)
	}
	defer rows.Close()
	records := make([]Record, 0)
	for rows.Next() {
		record, err := scanRecord(rows)
		if err != nil {
			return nil, signedServiceURLDatabaseError("scan signed service URL", err)
		}
		records = append(records, record)
	}
	if err := rows.Err(); err != nil {
		return nil, signedServiceURLDatabaseError("iterate signed service URLs", err)
	}
	return records, nil
}

func (store *PostgresStore) Get(ctx context.Context, id uuid.UUID) (Record, error) {
	record, err := scanRecord(store.pool.QueryRow(ctx, `SELECT `+recordColumns+` FROM signed_service_urls WHERE id = $1`, id))
	if errors.Is(err, pgx.ErrNoRows) {
		return Record{}, ErrNotFound
	}
	if err != nil {
		return Record{}, signedServiceURLDatabaseError("get signed service URL", err)
	}
	return record, nil
}

func (store *PostgresStore) Revoke(ctx context.Context, namespace string, id uuid.UUID, revokedAt time.Time) (Record, error) {
	record, err := scanRecord(store.pool.QueryRow(ctx, `UPDATE signed_service_urls SET revoked_at = COALESCE(revoked_at, $3) WHERE namespace = $1 AND id = $2 RETURNING `+recordColumns, namespace, id, revokedAt))
	if errors.Is(err, pgx.ErrNoRows) {
		return Record{}, ErrNotFound
	}
	if err != nil {
		return Record{}, signedServiceURLDatabaseError("revoke signed service URL", err)
	}
	return record, nil
}

type rowScanner interface{ Scan(...any) error }

func scanRecord(row rowScanner) (Record, error) {
	var record Record
	err := row.Scan(&record.ID, &record.Namespace, &record.ClaimName, &record.SandboxName, &record.ServiceName, &record.LogicalService, &record.Label, &record.CreatorSub, &record.CreatedAt, &record.ExpiresAt, &record.RevokedAt)
	return record, err
}

func signedServiceURLDatabaseError(operation string, err error) error {
	var postgresError *pgconn.PgError
	if errors.As(err, &postgresError) && !signedServiceURLUnavailableSQLState(postgresError.Code) {
		return fmt.Errorf("%s: %w", operation, err)
	}
	return fmt.Errorf("%w: %s: %w", ErrUnavailable, operation, err)
}

func signedServiceURLUnavailableSQLState(code string) bool {
	return code == "42P01" || len(code) >= 2 && (code[:2] == "08" || code[:2] == "28" || code[:2] == "53") || code == "57P01" || code == "57P02" || code == "57P03"
}
