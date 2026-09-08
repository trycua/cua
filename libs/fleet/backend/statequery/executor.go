package statequery

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

var ErrExecution = errors.New("state query execution failed")

type ResultWriter interface {
	WriteFieldDescriptions([]pgconn.FieldDescription) error
	WriteRow([]any) error
}

type Executor struct {
	baseConfig     *pgx.ConnConfig
	tenantPassword string
}

func TenantRoleName(tenant string) string {
	digest := sha256.Sum256([]byte(tenant))
	return "k8s_tenant_" + hex.EncodeToString(digest[:16])
}

func NewExecutor(queryDSN, tenantPassword string) (*Executor, error) {
	if queryDSN == "" || tenantPassword == "" {
		return nil, fmt.Errorf("state query DSN and tenant password are required")
	}
	config, err := pgx.ParseConfig(queryDSN)
	if err != nil {
		return nil, fmt.Errorf("parse state query DSN: %w", err)
	}
	return &Executor{baseConfig: config, tenantPassword: tenantPassword}, nil
}

func (e *Executor) connectionConfig(tenant string) *pgx.ConnConfig {
	config := e.baseConfig.Copy()
	config.User = TenantRoleName(tenant)
	config.Password = e.tenantPassword
	return config
}

func (e *Executor) Execute(ctx context.Context, tenant, sql string, writer ResultWriter) error {
	if tenant == "" {
		return fmt.Errorf("%w: tenant is required", ErrExecution)
	}
	conn, err := pgx.ConnectConfig(ctx, e.connectionConfig(tenant))
	if err != nil {
		return errors.Join(fmt.Errorf("%w: connect tenant role: %v", ErrExecution, err), err)

	}
	defer conn.Close(ctx)

	tx, err := conn.BeginTx(ctx, pgx.TxOptions{AccessMode: pgx.ReadOnly})
	if err != nil {
		return errors.Join(fmt.Errorf("%w: begin transaction", ErrExecution), err)

	}
	defer tx.Rollback(ctx)

	rows, err := tx.Query(ctx, sql)
	if err != nil {
		return errors.Join(fmt.Errorf("%w: execute query", ErrExecution), err)

	}
	defer rows.Close()
	if err := writer.WriteFieldDescriptions(rows.FieldDescriptions()); err != nil {
		return err
	}
	for rows.Next() {
		values, err := rows.Values()
		if err != nil {
			return errors.Join(fmt.Errorf("%w: decode result row", ErrExecution), err)

		}
		if err := writer.WriteRow(values); err != nil {
			return err
		}
	}
	rows.Close()
	return rows.Err()
}
