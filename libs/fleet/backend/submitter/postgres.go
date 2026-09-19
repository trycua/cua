package submitter

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

const submitterRole = "cyclops_submitter"

const submitterLockKey = "cyclops-usage-submitter"

const tryLockStatement = `select pg_try_advisory_lock(hashtextextended($1, 0))`

const unlockStatement = `select pg_advisory_unlock(hashtextextended($1, 0))`

const latestLiveCutoverStatement = `select cutover_hour
	from billing_meter.stripe_submission_batch
	where dry_run = false
	order by cutover_hour desc
	limit 1`

const completedHoursStatement = `select hour_start
	from billing_meter.reservation_hour_collection_current
	where cluster_id = $1 and hour_start >= $2 and hour_start < $3
	order by hour_start`

// Quantities are computed in Postgres numeric and truncated once, per the
// CUA-1150 contract: one unit is 1 vCPU or 1 GiB (2^30 bytes) for 3600 s.
const aggregateTenantHoursStatement = `select
	capsule_tenant,
	hour_start,
	trunc(sum(virtual_cpu_core_seconds) / 3600, 6)::text,
	trunc(sum(virtual_memory_byte_seconds) / (1073741824::numeric * 3600), 6)::text,
	array_agg(fact_id::text order by fact_id),
	array_agg(revision order by fact_id),
	array_agg(virtual_cpu_core_seconds::text order by fact_id),
	array_agg(virtual_memory_byte_seconds::text order by fact_id)
	from billing_meter.reservation_hour_current
	where cluster_id = $1 and hour_start = any($2)
	group by capsule_tenant, hour_start
	order by capsule_tenant, hour_start`

const ledgerHeadsStatement = `select distinct on (capsule_tenant, hour_start, meter)
	submission_id, batch_id, capsule_tenant, hour_start, meter, seq, status, fact_set_sha256
	from billing_meter.stripe_submission
	where hour_start = any($1)
	order by capsule_tenant, hour_start, meter, seq desc`

const insertBatchStatement = `insert into billing_meter.stripe_submission_batch (
	batch_id, cluster_id, cutover_hour, window_start, window_end, dry_run,
	archive_bucket, archive_key, submission_count, created_at
) values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)`

const lockTenantStatement = `select pg_advisory_xact_lock(hashtextextended($1, 0))`

const nextSeqStatement = `select coalesce(max(seq), 0) + 1
	from billing_meter.stripe_submission
	where capsule_tenant = $1 and hour_start = $2 and meter = $3`

const insertSubmissionStatement = `insert into billing_meter.stripe_submission (
	submission_id, batch_id, capsule_tenant, hour_start, meter, seq, identifier,
	stripe_customer_id, quantity, fact_set_sha256, status, created_at
) values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)`

const incompleteBatchesStatement = `select
	batch_id, cluster_id, cutover_hour, window_start, window_end, dry_run,
	archive_bucket, archive_key, coalesce(archive_sha256, ''), submission_count,
	created_at, archived_at, completed_at
	from billing_meter.stripe_submission_batch
	where completed_at is null
	order by created_at`

const batchSubmissionsStatement = `select
	submission_id, batch_id, capsule_tenant, hour_start, meter, seq, identifier,
	coalesce(stripe_customer_id, ''), quantity::text, fact_set_sha256, status, created_at
	from billing_meter.stripe_submission
	where batch_id = $1
	order by identifier`

const markBatchArchivedStatement = `update billing_meter.stripe_submission_batch
	set archive_sha256 = $2, archived_at = $3
	where batch_id = $1 and archived_at is null`

const markBatchCompletedStatement = `update billing_meter.stripe_submission_batch
	set completed_at = $2
	where batch_id = $1 and completed_at is null`

const markSubmissionStatement = `update billing_meter.stripe_submission
	set status = $2, stripe_event_id = nullif($3, ''), error_code = nullif($4, ''),
	    error_message = nullif($5, ''), sent_at = $6
	where submission_id = $1 and status = 'archived'`

type PostgresStore struct {
	pool *pgxpool.Pool
}

func NewPostgresStore(ctx context.Context, databaseURL string) (*PostgresStore, error) {
	config, err := pgxpool.ParseConfig(databaseURL)
	if err != nil {
		return nil, fmt.Errorf("parse submitter database URL: %w", err)
	}
	if config.ConnConfig.User != submitterRole {
		return nil, fmt.Errorf("submitter database URL must use %s", submitterRole)
	}
	config.MaxConns = 2
	config.MinConns = 0
	config.MaxConnIdleTime = time.Minute
	config.MaxConnLifetime = 15 * time.Minute
	pool, err := pgxpool.NewWithConfig(ctx, config)
	if err != nil {
		return nil, fmt.Errorf("open submitter database pool: %w", err)
	}
	return &PostgresStore{pool: pool}, nil
}

func (s *PostgresStore) Close() {
	if s != nil && s.pool != nil {
		s.pool.Close()
	}
}

// TryLock holds a session advisory lock on a dedicated connection for the
// life of the run, so two overlapping runs cannot both create batches.
func (s *PostgresStore) TryLock(ctx context.Context) (bool, func(context.Context) error, error) {
	connection, err := s.pool.Acquire(ctx)
	if err != nil {
		return false, nil, fmt.Errorf("acquire lock connection: %w", err)
	}
	var locked bool
	if err := connection.QueryRow(ctx, tryLockStatement, submitterLockKey).Scan(&locked); err != nil {
		connection.Release()
		return false, nil, fmt.Errorf("try submitter lock: %w", err)
	}
	if !locked {
		connection.Release()
		return false, func(context.Context) error { return nil }, nil
	}
	unlock := func(ctx context.Context) error {
		defer connection.Release()
		var released bool
		if err := connection.QueryRow(ctx, unlockStatement, submitterLockKey).Scan(&released); err != nil {
			return fmt.Errorf("release submitter lock: %w", err)
		}
		if !released {
			return errors.New("submitter lock was not held at release")
		}
		return nil
	}
	return true, unlock, nil
}

func (s *PostgresStore) LatestLiveCutover(ctx context.Context) (time.Time, bool, error) {
	var cutover time.Time
	err := s.pool.QueryRow(ctx, latestLiveCutoverStatement).Scan(&cutover)
	if errors.Is(err, pgx.ErrNoRows) {
		return time.Time{}, false, nil
	}
	if err != nil {
		return time.Time{}, false, fmt.Errorf("query live cutover: %w", err)
	}
	return cutover.UTC(), true, nil
}

func (s *PostgresStore) CompletedHours(ctx context.Context, clusterID string, from, to time.Time) ([]time.Time, error) {
	rows, err := s.pool.Query(ctx, completedHoursStatement, clusterID, from, to)
	if err != nil {
		return nil, fmt.Errorf("query completed hours: %w", err)
	}
	defer rows.Close()
	var hours []time.Time
	for rows.Next() {
		var hour time.Time
		if err := rows.Scan(&hour); err != nil {
			return nil, fmt.Errorf("scan completed hour: %w", err)
		}
		hours = append(hours, hour.UTC())
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate completed hours: %w", err)
	}
	return hours, nil
}

func (s *PostgresStore) AggregateTenantHours(ctx context.Context, clusterID string, hours []time.Time) ([]TenantHour, error) {
	rows, err := s.pool.Query(ctx, aggregateTenantHoursStatement, clusterID, hours)
	if err != nil {
		return nil, fmt.Errorf("query tenant hours: %w", err)
	}
	defer rows.Close()
	var result []TenantHour
	for rows.Next() {
		var tenantHour TenantHour
		var factIDs, cpuSeconds, memorySeconds []string
		var revisions []int32
		if err := rows.Scan(&tenantHour.Tenant, &tenantHour.HourStart, &tenantHour.VCPUHours, &tenantHour.GiBHours, &factIDs, &revisions, &cpuSeconds, &memorySeconds); err != nil {
			return nil, fmt.Errorf("scan tenant hour: %w", err)
		}
		if len(factIDs) != len(revisions) || len(factIDs) != len(cpuSeconds) || len(factIDs) != len(memorySeconds) {
			return nil, fmt.Errorf("tenant hour %s/%s returned mismatched fact arrays", tenantHour.Tenant, tenantHour.HourStart.Format(time.RFC3339))
		}
		tenantHour.HourStart = tenantHour.HourStart.UTC()
		for index := range factIDs {
			tenantHour.Facts = append(tenantHour.Facts, FactRef{
				FactID:            factIDs[index],
				Revision:          int(revisions[index]),
				CPUCoreSeconds:    cpuSeconds[index],
				MemoryByteSeconds: memorySeconds[index],
			})
		}
		result = append(result, tenantHour)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate tenant hours: %w", err)
	}
	return result, nil
}

func (s *PostgresStore) LedgerHeads(ctx context.Context, hours []time.Time) (map[LedgerKey]LedgerHead, error) {
	rows, err := s.pool.Query(ctx, ledgerHeadsStatement, hours)
	if err != nil {
		return nil, fmt.Errorf("query ledger heads: %w", err)
	}
	defer rows.Close()
	heads := map[LedgerKey]LedgerHead{}
	for rows.Next() {
		var key LedgerKey
		var head LedgerHead
		if err := rows.Scan(&head.SubmissionID, &head.BatchID, &key.Tenant, &key.HourStart, &key.Meter, &head.Seq, &head.Status, &head.FactSetSHA256); err != nil {
			return nil, fmt.Errorf("scan ledger head: %w", err)
		}
		key.HourStart = key.HourStart.UTC()
		heads[key] = head
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate ledger heads: %w", err)
	}
	return heads, nil
}

// CreateBatch inserts the batch and its submissions in one transaction. Seq
// values are re-derived under a per-tenant advisory lock so a concurrent
// writer (which the run lock should already prevent) cannot collide.
func (s *PostgresStore) CreateBatch(ctx context.Context, batch Batch, submissions []Submission) ([]Submission, error) {
	transaction, err := s.pool.BeginTx(ctx, pgx.TxOptions{})
	if err != nil {
		return nil, fmt.Errorf("begin batch transaction: %w", err)
	}
	defer func() { _ = transaction.Rollback(ctx) }()

	if _, err := transaction.Exec(ctx, insertBatchStatement,
		batch.BatchID, batch.ClusterID, batch.CutoverHour, batch.WindowStart, batch.WindowEnd, batch.DryRun,
		batch.ArchiveBucket, batch.ArchiveKey, len(submissions), batch.CreatedAt); err != nil {
		return nil, fmt.Errorf("insert batch: %w", err)
	}
	created := make([]Submission, 0, len(submissions))
	for _, submission := range submissions {
		if _, err := transaction.Exec(ctx, lockTenantStatement, submission.Tenant); err != nil {
			return nil, fmt.Errorf("lock tenant %q: %w", submission.Tenant, err)
		}
		var seq int
		if err := transaction.QueryRow(ctx, nextSeqStatement, submission.Tenant, submission.HourStart, submission.Meter).Scan(&seq); err != nil {
			return nil, fmt.Errorf("read next seq for %s: %w", submission.Identifier, err)
		}
		if seq != submission.Seq {
			return nil, fmt.Errorf("seq for %s changed underneath the run: planned %d, ledger says %d", submission.Identifier, submission.Seq, seq)
		}
		var customer any
		if submission.StripeCustomerID != "" {
			customer = submission.StripeCustomerID
		}
		if _, err := transaction.Exec(ctx, insertSubmissionStatement,
			submission.SubmissionID, submission.BatchID, submission.Tenant, submission.HourStart, submission.Meter,
			submission.Seq, submission.Identifier, customer, submission.Quantity, submission.FactSetSHA256,
			submission.Status, submission.CreatedAt); err != nil {
			return nil, fmt.Errorf("insert submission %s: %w", submission.Identifier, err)
		}
		created = append(created, submission)
	}
	if err := transaction.Commit(ctx); err != nil {
		return nil, fmt.Errorf("commit batch: %w", err)
	}
	return created, nil
}

func (s *PostgresStore) IncompleteBatches(ctx context.Context) ([]Batch, error) {
	rows, err := s.pool.Query(ctx, incompleteBatchesStatement)
	if err != nil {
		return nil, fmt.Errorf("query incomplete batches: %w", err)
	}
	defer rows.Close()
	var batches []Batch
	for rows.Next() {
		var batch Batch
		var archivedAt, completedAt *time.Time
		if err := rows.Scan(&batch.BatchID, &batch.ClusterID, &batch.CutoverHour, &batch.WindowStart, &batch.WindowEnd, &batch.DryRun,
			&batch.ArchiveBucket, &batch.ArchiveKey, &batch.ArchiveSHA256, &batch.SubmissionCount,
			&batch.CreatedAt, &archivedAt, &completedAt); err != nil {
			return nil, fmt.Errorf("scan incomplete batch: %w", err)
		}
		if archivedAt != nil {
			batch.ArchivedAt = archivedAt.UTC()
		}
		if completedAt != nil {
			batch.CompletedAt = completedAt.UTC()
		}
		batches = append(batches, batch)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate incomplete batches: %w", err)
	}
	return batches, nil
}

func (s *PostgresStore) BatchSubmissions(ctx context.Context, batchID uuid.UUID) ([]Submission, error) {
	rows, err := s.pool.Query(ctx, batchSubmissionsStatement, batchID)
	if err != nil {
		return nil, fmt.Errorf("query batch submissions: %w", err)
	}
	defer rows.Close()
	var submissions []Submission
	for rows.Next() {
		var submission Submission
		if err := rows.Scan(&submission.SubmissionID, &submission.BatchID, &submission.Tenant, &submission.HourStart, &submission.Meter,
			&submission.Seq, &submission.Identifier, &submission.StripeCustomerID, &submission.Quantity, &submission.FactSetSHA256,
			&submission.Status, &submission.CreatedAt); err != nil {
			return nil, fmt.Errorf("scan batch submission: %w", err)
		}
		submission.HourStart = submission.HourStart.UTC()
		submissions = append(submissions, submission)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate batch submissions: %w", err)
	}
	return submissions, nil
}

func (s *PostgresStore) MarkBatchArchived(ctx context.Context, batchID uuid.UUID, sha256 string, at time.Time) error {
	tag, err := s.pool.Exec(ctx, markBatchArchivedStatement, batchID, sha256, at)
	if err != nil {
		return fmt.Errorf("mark batch %s archived: %w", batchID, err)
	}
	if tag.RowsAffected() != 1 {
		return fmt.Errorf("batch %s was not in the unarchived state", batchID)
	}
	return nil
}

func (s *PostgresStore) MarkBatchCompleted(ctx context.Context, batchID uuid.UUID, at time.Time) error {
	tag, err := s.pool.Exec(ctx, markBatchCompletedStatement, batchID, at)
	if err != nil {
		return fmt.Errorf("mark batch %s completed: %w", batchID, err)
	}
	if tag.RowsAffected() != 1 {
		return fmt.Errorf("batch %s was not in the incomplete state", batchID)
	}
	return nil
}

func (s *PostgresStore) MarkSubmission(ctx context.Context, submissionID uuid.UUID, outcome Outcome) error {
	var sentAt any
	if !outcome.SentAt.IsZero() {
		sentAt = outcome.SentAt
	}
	tag, err := s.pool.Exec(ctx, markSubmissionStatement, submissionID, outcome.Status, outcome.StripeEventID, outcome.ErrorCode, outcome.ErrorMessage, sentAt)
	if err != nil {
		return fmt.Errorf("mark submission %s %s: %w", submissionID, outcome.Status, err)
	}
	if tag.RowsAffected() != 1 {
		return fmt.Errorf("submission %s was not in the archived state", submissionID)
	}
	return nil
}
