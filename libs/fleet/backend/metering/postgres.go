package metering

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
)

const lockReservationHourCompletionStatement = `select pg_advisory_xact_lock(hashtextextended($1, 0))`

const selectReservationHourCompletionStatement = `select
	coalesce(current.collection_run_id, '00000000-0000-0000-0000-000000000000'::uuid),
	coalesce(current.revision, 0),
	coalesce(current.source_sha256, '')
	from (values (1)) as seed(value)
	left join billing_meter.reservation_hour_collection_current as current on current.logical_key = $1`

const insertReservationHourCompletionStatement = `insert into billing_meter.reservation_hour_collection (
	collection_run_id, logical_key, revision, cluster_id, hour_start, hour_end,
	covered_seconds, discovered_sandboxes, inserted_facts, unchanged_facts,
	source_sha256, supersedes_collection_run_id
) values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)`

type PostgresStore struct {
	pool *pgxpool.Pool
}

func NewPostgresStore(ctx context.Context, databaseURL string) (*PostgresStore, error) {
	config, err := pgxpool.ParseConfig(databaseURL)
	if err != nil {
		return nil, fmt.Errorf("parse meter database URL: %w", err)
	}
	if config.ConnConfig.User != "cyclops_meter_writer" {
		return nil, fmt.Errorf("meter database URL must use cyclops_meter_writer")
	}
	config.MaxConns = 2
	config.MinConns = 0
	config.MaxConnIdleTime = time.Minute
	config.MaxConnLifetime = 15 * time.Minute
	pool, err := pgxpool.NewWithConfig(ctx, config)
	if err != nil {
		return nil, fmt.Errorf("open meter database pool: %w", err)
	}
	return &PostgresStore{pool: pool}, nil
}

func (s *PostgresStore) Close() {
	if s != nil && s.pool != nil {
		s.pool.Close()
	}
}

func (s *PostgresStore) ResolveTenant(ctx context.Context, clusterID, namespace, sandboxUID string) (string, bool, error) {
	var tenant string
	if err := s.pool.QueryRow(ctx, `select k8s_api.sandbox_meter_tenant($1, $2, $3)`, clusterID, namespace, sandboxUID).Scan(&tenant); err != nil {
		var postgresError *pgconn.PgError
		if errors.As(err, &postgresError) && postgresError.Code == "P0001" && postgresError.Message == "sandbox tenant identity is unavailable" {
			return "", false, nil
		}
		return "", false, fmt.Errorf("query sandbox tenant: %w", err)
	}
	if strings.TrimSpace(tenant) == "" {
		return "", false, fmt.Errorf("sandbox tenant is empty")
	}
	return tenant, true, nil
}

func (s *PostgresStore) AppendFact(ctx context.Context, fact HourFact) (bool, error) {
	if err := validateHourFact(fact); err != nil {
		return false, err
	}
	transaction, err := s.pool.BeginTx(ctx, pgx.TxOptions{})
	if err != nil {
		return false, fmt.Errorf("begin fact transaction: %w", err)
	}
	defer func() { _ = transaction.Rollback(ctx) }()
	if _, err := transaction.Exec(ctx, `select pg_advisory_xact_lock(hashtextextended($1, 0))`, fact.LogicalKey); err != nil {
		return false, fmt.Errorf("lock reservation fact: %w", err)
	}

	var previousID uuid.UUID
	var previousRevision int
	var previousSHA string
	queryErr := transaction.QueryRow(ctx, `select
		coalesce(current.fact_id, '00000000-0000-0000-0000-000000000000'::uuid),
		coalesce(current.revision, 0),
		coalesce(current.source_sha256, '')
		from (values (1)) as seed(value)
		left join billing_meter.reservation_hour_current as current on current.logical_key = $1`, fact.LogicalKey).
		Scan(&previousID, &previousRevision, &previousSHA)
	if queryErr != nil {
		return false, fmt.Errorf("read current reservation fact: %w", queryErr)
	}
	if previousRevision > 0 && previousSHA == fact.SourceSHA256 {
		if err := transaction.Commit(ctx); err != nil {
			return false, fmt.Errorf("commit unchanged reservation fact: %w", err)
		}
		return false, nil
	}

	revision := previousRevision + 1
	var supersedes any
	if previousRevision > 0 {
		supersedes = previousID
	}
	_, insertErr := transaction.Exec(ctx, `insert into billing_meter.reservation_hour_fact (
		fact_id, logical_key, revision, cluster_id, capsule_tenant, namespace,
		sandbox_uid, sandbox_name, pool_name, runtime, hour_start, hour_end,
		virtual_cpu_core_seconds, virtual_memory_byte_seconds, ready_seconds,
		covered_seconds, scrape_interval_seconds, source_sha256, collection_run_id,
		supersedes_fact_id
	) values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)`,
		fact.FactID, fact.LogicalKey, revision, fact.ClusterID, fact.CapsuleTenant, fact.Namespace,
		fact.SandboxUID, fact.SandboxName, fact.PoolName, fact.Runtime, fact.HourStart, fact.HourEnd,
		fact.VirtualCPUCoreSeconds, fact.VirtualMemoryByteSeconds, fact.ReadySeconds,
		fact.CoveredSeconds, fact.ScrapeIntervalSeconds, fact.SourceSHA256, fact.CollectionRunID,
		supersedes)
	if insertErr != nil {
		return false, fmt.Errorf("insert reservation fact: %w", insertErr)
	}
	if err := transaction.Commit(ctx); err != nil {
		return false, fmt.Errorf("commit reservation fact: %w", err)
	}
	return true, nil
}

func (s *PostgresStore) CompleteHour(ctx context.Context, completion HourCompletion) (bool, error) {
	if err := validateHourCompletion(completion); err != nil {
		return false, err
	}
	transaction, err := s.pool.BeginTx(ctx, pgx.TxOptions{})
	if err != nil {
		return false, fmt.Errorf("begin hour completion transaction: %w", err)
	}
	defer func() { _ = transaction.Rollback(ctx) }()
	if _, err := transaction.Exec(ctx, lockReservationHourCompletionStatement, completion.LogicalKey); err != nil {
		return false, fmt.Errorf("lock reservation hour completion: %w", err)
	}
	var previousID uuid.UUID
	var previousRevision int
	var previousSHA string
	queryErr := transaction.QueryRow(ctx, selectReservationHourCompletionStatement, completion.LogicalKey).
		Scan(&previousID, &previousRevision, &previousSHA)
	if queryErr != nil {
		return false, fmt.Errorf("read current hour completion: %w", queryErr)
	}
	if previousRevision > 0 && previousSHA == completion.SourceSHA256 {
		if err := transaction.Commit(ctx); err != nil {
			return false, fmt.Errorf("commit unchanged hour completion: %w", err)
		}
		return false, nil
	}
	revision := previousRevision + 1
	var supersedes any
	if previousRevision > 0 {
		supersedes = previousID
	}
	_, insertErr := transaction.Exec(ctx, insertReservationHourCompletionStatement,
		completion.CollectionRunID, completion.LogicalKey, revision, completion.ClusterID,
		completion.HourStart, completion.HourEnd, completion.CoveredSeconds,
		completion.DiscoveredSandboxes, completion.InsertedFacts, completion.UnchangedFacts,
		completion.SourceSHA256, supersedes)
	if insertErr != nil {
		return false, fmt.Errorf("insert hour completion: %w", insertErr)
	}
	if err := transaction.Commit(ctx); err != nil {
		return false, fmt.Errorf("commit hour completion: %w", err)
	}
	return true, nil
}

func validateHourCompletion(completion HourCompletion) error {
	if completion.CollectionRunID == uuid.Nil || completion.LogicalKey == "" || completion.ClusterID == "" || len(completion.SourceSHA256) != 64 {
		return fmt.Errorf("hour completion has empty required fields")
	}
	if !completion.HourEnd.Equal(completion.HourStart.Add(time.Hour)) || !completion.HourStart.Equal(completion.HourStart.UTC().Truncate(time.Hour)) {
		return fmt.Errorf("hour completion has invalid hour")
	}
	if completion.CoveredSeconds < 0 || completion.CoveredSeconds > 3600 || completion.DiscoveredSandboxes < 0 || completion.InsertedFacts < 0 || completion.UnchangedFacts < 0 {
		return fmt.Errorf("hour completion has invalid quantities")
	}
	return nil
}

func validateHourFact(fact HourFact) error {
	if fact.FactID == uuid.Nil || fact.CollectionRunID == uuid.Nil || fact.LogicalKey == "" || fact.ClusterID == "" || fact.CapsuleTenant == "" || fact.Namespace == "" || fact.SandboxUID == "" || fact.SandboxName == "" || fact.PoolName == "" || fact.Runtime == "" {
		return fmt.Errorf("reservation fact has empty required fields")
	}
	if !fact.HourEnd.Equal(fact.HourStart.Add(time.Hour)) || !fact.HourStart.Equal(fact.HourStart.UTC().Truncate(time.Hour)) {
		return fmt.Errorf("reservation fact has invalid hour")
	}
	if len(fact.SourceSHA256) != 64 || fact.ScrapeIntervalSeconds <= 0 || fact.VirtualCPUCoreSeconds < 0 || fact.VirtualMemoryByteSeconds < 0 || fact.ReadySeconds < 0 || fact.CoveredSeconds < 0 || fact.CoveredSeconds > 3600 {
		return fmt.Errorf("reservation fact has invalid quantities")
	}
	return nil
}

var _ TenantResolver = (*PostgresStore)(nil)
var _ FactWriter = (*PostgresStore)(nil)
