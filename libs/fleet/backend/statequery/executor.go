package statequery

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

var ErrExecution = errors.New("state query execution failed")

type Column struct {
	Name string `json:"name"`
	Type string `json:"type"`
}

type ResultSet struct {
	Columns     []Column `json:"columns"`
	Rows        [][]any  `json:"rows"`
	Truncated   bool     `json:"truncated"`
	DurationMS  int64    `json:"duration_ms"`
	Checkpoints []any    `json:"checkpoints"`
}

type Executor struct {
	queryPool     *pgxpool.Pool
	roleAdminPool *pgxpool.Pool
}

func TenantRoleName(tenant string) string {
	digest := sha256.Sum256([]byte(tenant))
	return "k8s_tenant_" + hex.EncodeToString(digest[:16])
}

func NewExecutor(ctx context.Context, queryURL, roleAdminURL string) (*Executor, error) {
	if queryURL == "" || roleAdminURL == "" {
		return nil, fmt.Errorf("state query database URLs are required")
	}
	queryPool, err := pgxpool.New(ctx, queryURL)
	if err != nil {
		return nil, fmt.Errorf("open state query pool: %w", err)
	}
	roleAdminPool, err := pgxpool.New(ctx, roleAdminURL)
	if err != nil {
		queryPool.Close()
		return nil, fmt.Errorf("open state role-admin pool: %w", err)
	}
	if err := queryPool.Ping(ctx); err != nil {
		queryPool.Close()
		roleAdminPool.Close()
		return nil, fmt.Errorf("ping state query pool: %w", err)
	}
	if err := roleAdminPool.Ping(ctx); err != nil {
		queryPool.Close()
		roleAdminPool.Close()
		return nil, fmt.Errorf("ping state role-admin pool: %w", err)
	}
	return &Executor{queryPool: queryPool, roleAdminPool: roleAdminPool}, nil
}

func (e *Executor) Close() {
	e.queryPool.Close()
	e.roleAdminPool.Close()
}

func (e *Executor) Execute(ctx context.Context, tenant string, admin bool, query ValidatedQuery) (ResultSet, error) {
	role := "k8s_query_admin"
	if !admin {
		if tenant == "" {
			return ResultSet{}, fmt.Errorf("%w: provision tenant role", ErrExecution)
		}
		role = TenantRoleName(tenant)
		if err := e.ensureTenantRole(ctx, role, tenant); err != nil {
			return ResultSet{}, fmt.Errorf("%w: provision tenant role", ErrExecution)
		}
	}

	started := time.Now()
	tx, err := e.queryPool.BeginTx(ctx, pgx.TxOptions{AccessMode: pgx.ReadOnly})
	if err != nil {
		return ResultSet{}, fmt.Errorf("%w: begin transaction", ErrExecution)
	}
	defer tx.Rollback(ctx)
	if _, err := tx.Exec(ctx, "set local role "+pgx.Identifier{role}.Sanitize()); err != nil {
		return ResultSet{}, fmt.Errorf("%w: assume query role", ErrExecution)
	}
	for setting, value := range map[string]string{
		"statement_timeout":                   fmt.Sprintf("%dms", query.TimeoutMS),
		"lock_timeout":                        "500ms",
		"idle_in_transaction_session_timeout": "10s",
		"work_mem":                            "16MB",
	} {
		if _, err := tx.Exec(ctx, `select set_config($1, $2, true)`, setting, value); err != nil {
			return ResultSet{}, fmt.Errorf("%w: apply query limits", ErrExecution)
		}
	}

	rows, err := tx.Query(ctx, query.SQL)
	if err != nil {
		return ResultSet{}, fmt.Errorf("%w: execute select", ErrExecution)
	}
	defer rows.Close()
	fields := rows.FieldDescriptions()
	result := ResultSet{
		Columns:     make([]Column, len(fields)),
		Rows:        make([][]any, 0, query.MaxRows),
		Checkpoints: []any{},
	}
	for index, field := range fields {
		result.Columns[index] = Column{Name: field.Name, Type: postgresTypeName(field.DataTypeOID)}
	}
	for rows.Next() {
		values, err := rows.Values()
		if err != nil {
			return ResultSet{}, fmt.Errorf("%w: decode result row", ErrExecution)
		}
		if len(result.Rows) == query.MaxRows {
			result.Truncated = true
			break
		}
		result.Rows = append(result.Rows, values)
	}
	if rows.Err() != nil {
		return ResultSet{}, fmt.Errorf("%w: stream result rows", ErrExecution)
	}
	result.DurationMS = time.Since(started).Milliseconds()
	return result, nil
}

func (e *Executor) ensureTenantRole(ctx context.Context, role, tenant string) error {
	tx, err := e.roleAdminPool.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx)
	if _, err := tx.Exec(ctx, `select pg_advisory_xact_lock(hashtext($1))`, "cyclops-state-role:"+role); err != nil {
		return err
	}
	var exists bool
	if err := tx.QueryRow(ctx, `select exists(select 1 from pg_roles where rolname = $1)`, role).Scan(&exists); err != nil {
		return err
	}
	identifier := pgx.Identifier{role}.Sanitize()
	if !exists {
		if _, err := tx.Exec(ctx, "create role "+identifier+" nologin inherit nocreaterole"); err != nil {
			return err
		}
	}
	if _, err := tx.Exec(ctx, "grant k8s_query_tenant to "+identifier); err != nil {
		return err
	}
	if _, err := tx.Exec(ctx, "grant "+identifier+" to k8s_query_broker"); err != nil {
		return err
	}
	if _, err := tx.Exec(ctx, `select k8s_state.register_tenant_role($1, $2)`, role, tenant); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

func postgresTypeName(oid uint32) string {
	switch oid {
	case 16:
		return "bool"
	case 20:
		return "int8"
	case 23:
		return "int4"
	case 25:
		return "text"
	case 114:
		return "json"
	case 1184:
		return "timestamptz"
	case 3802:
		return "jsonb"
	default:
		return fmt.Sprintf("oid:%d", oid)
	}
}
