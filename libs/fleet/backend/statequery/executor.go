package statequery

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"time"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"

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

func finishDatabaseSpan(span trace.Span, err error) {
	if err != nil {
		span.SetAttributes(attribute.String("error.type", fmt.Sprintf("%T", err)))
		var postgresErr *pgconn.PgError
		if errors.As(err, &postgresErr) {
			span.SetAttributes(attribute.String("db.postgresql.sqlstate", postgresErr.Code))
		}
		span.SetStatus(codes.Error, "database operation failed")
	}
	span.End()
}

func (e *Executor) Execute(ctx context.Context, tenant, sql string, writer ResultWriter) (resultErr error) {
	tracer := otel.Tracer("cyclops-cs-backend/statequery")
	ctx, span := tracer.Start(ctx, "db.execute", trace.WithAttributes(attribute.String("db.system", "postgresql")))
	defer func() { finishDatabaseSpan(span, resultErr) }()
	if tenant == "" {
		return fmt.Errorf("%w: tenant is required", ErrExecution)
	}
	connectCtx, connectSpan := tracer.Start(ctx, "db.connect", trace.WithSpanKind(trace.SpanKindClient))
	conn, err := pgx.ConnectConfig(connectCtx, e.connectionConfig(tenant))
	finishDatabaseSpan(connectSpan, err)
	if err != nil {
		return errors.Join(fmt.Errorf("%w: connect tenant role: %v", ErrExecution, err), err)

	}
	defer func() {
		closeCtx, closeSpan := tracer.Start(context.WithoutCancel(ctx), "db.connection.close")
		closeCtx, cancel := context.WithTimeout(closeCtx, 3*time.Second)
		defer cancel()
		finishDatabaseSpan(closeSpan, conn.Close(closeCtx))
	}()

	beginCtx, beginSpan := tracer.Start(ctx, "db.transaction.begin", trace.WithAttributes(attribute.Bool("db.transaction.read_only", true)))
	tx, err := conn.BeginTx(beginCtx, pgx.TxOptions{AccessMode: pgx.ReadOnly})
	finishDatabaseSpan(beginSpan, err)
	if err != nil {
		return errors.Join(fmt.Errorf("%w: begin transaction", ErrExecution), err)

	}
	defer func() {
		rollbackCtx, rollbackSpan := tracer.Start(context.WithoutCancel(ctx), "db.transaction.rollback")
		rollbackCtx, cancel := context.WithTimeout(rollbackCtx, 3*time.Second)
		defer cancel()
		finishDatabaseSpan(rollbackSpan, tx.Rollback(rollbackCtx))
	}()

	queryCtx, querySpan := tracer.Start(ctx, "db.query", trace.WithSpanKind(trace.SpanKindClient))
	rows, err := tx.Query(queryCtx, sql)
	finishDatabaseSpan(querySpan, err)
	if err != nil {
		return errors.Join(fmt.Errorf("%w: execute query", ErrExecution), err)

	}
	_, rowsSpan := tracer.Start(ctx, "db.rows.read_decode")
	rowCount := 0
	defer func() {
		rows.Close()
		rowsSpan.SetAttributes(attribute.Int("db.response.returned_rows", rowCount))
		finishDatabaseSpan(rowsSpan, resultErr)
	}()
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
		rowCount++
	}
	return rows.Err()
}
