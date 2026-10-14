// Package submitter turns completed reservation hours in billing_meter into
// Stripe meter events, exactly once per tenant-hour-meter, with a durable
// local ledger and an S3 archive written before anything is sent.
//
// Design: docs/superpowers/specs/2026-09-16-cua-1158-stripe-usage-submitter-design.md
//
// This package currently implements the dry-run path only: it selects,
// aggregates, ledgers, and archives, then marks rows dry_run. The Stripe
// path is added behind DryRun=false in a later change.
package submitter

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/google/uuid"
)

const (
	MeterVCPUHours = "cua_vcpu_hours"
	MeterGiBHours  = "cua_gib_hours"

	StatusArchived    = "archived"
	StatusSent        = "sent"
	StatusDryRun      = "dry_run"
	StatusFailed      = "failed"
	StatusOutOfWindow = "out_of_window"
	StatusUnmapped    = "unmapped"

	// ErrorCodeSuperseded marks an in-flight row whose facts changed before it
	// was completed; a newer seq carries the recomputed quantity.
	ErrorCodeSuperseded = "superseded"

	// MaxIdentifierLength is Stripe's limit on meter event identifiers.
	MaxIdentifierLength = 100

	ArchiveDataset = "reservation-usage"
	SchemaVersion  = "v1"
)

var Meters = []string{MeterVCPUHours, MeterGiBHours}

type Config struct {
	ClusterID       string
	CutoverHour     time.Time
	SettlementLag   time.Duration
	Lookback        time.Duration
	DryRun          bool
	ArchiveBucket   string
	ArchivePrefix   string
	ExporterVersion string
}

func (c Config) validate() error {
	if strings.TrimSpace(c.ClusterID) == "" {
		return errors.New("cluster id is required")
	}
	if c.CutoverHour.IsZero() || !c.CutoverHour.Equal(c.CutoverHour.UTC().Truncate(time.Hour)) {
		return errors.New("cutover hour must be an exact UTC hour")
	}
	if c.SettlementLag <= 0 || c.Lookback <= 0 {
		return errors.New("settlement lag and lookback must be positive")
	}
	if strings.TrimSpace(c.ArchiveBucket) == "" {
		return errors.New("archive bucket is required")
	}
	if !c.DryRun {
		return errors.New("live submission is not implemented; DryRun must be true")
	}
	return nil
}

// FactRef is one current-revision fact row contributing to a tenant-hour.
type FactRef struct {
	FactID            string
	Revision          int
	CPUCoreSeconds    string
	MemoryByteSeconds string
}

// TenantHour is the aggregate the submitter bills: one tenant, one UTC hour,
// both meters. Quantities are decimal strings already truncated to six
// places by Postgres numeric arithmetic.
type TenantHour struct {
	Tenant    string
	HourStart time.Time
	VCPUHours string
	GiBHours  string
	Facts     []FactRef
}

func (h TenantHour) Quantity(meter string) (string, error) {
	switch meter {
	case MeterVCPUHours:
		return h.VCPUHours, nil
	case MeterGiBHours:
		return h.GiBHours, nil
	}
	return "", fmt.Errorf("unknown meter %q", meter)
}

// FactSetSHA256 is order-independent over (fact_id, value) for the meter's
// own dimension. The revision is deliberately excluded: a revision that
// changes only CPU seconds must not resubmit an identical memory quantity.
func FactSetSHA256(meter string, facts []FactRef) (string, error) {
	lines := make([]string, 0, len(facts))
	for _, fact := range facts {
		value := fact.CPUCoreSeconds
		switch meter {
		case MeterVCPUHours:
		case MeterGiBHours:
			value = fact.MemoryByteSeconds
		default:
			return "", fmt.Errorf("unknown meter %q", meter)
		}
		lines = append(lines, fact.FactID+":"+value)
	}
	sort.Strings(lines)
	digest := sha256.Sum256([]byte(strings.Join(lines, "\n")))
	return hex.EncodeToString(digest[:]), nil
}

// Identifier is the Stripe meter event identifier: deterministic per
// tenant-hour-meter-seq so retries dedupe and a new seq overwrites the hour.
func Identifier(meter, tenant string, hourStart time.Time, seq int) (string, error) {
	if seq <= 0 {
		return "", errors.New("seq must be positive")
	}
	identifier := fmt.Sprintf("%s:%s:%s:r%d", meter, tenant, hourStart.UTC().Format(time.RFC3339), seq)
	if len(identifier) > MaxIdentifierLength {
		return "", fmt.Errorf("identifier for tenant %q exceeds %d characters", tenant, MaxIdentifierLength)
	}
	return identifier, nil
}

// Submission mirrors a billing_meter.stripe_submission row.
type Submission struct {
	SubmissionID     uuid.UUID
	BatchID          uuid.UUID
	Tenant           string
	HourStart        time.Time
	Meter            string
	Seq              int
	Identifier       string
	StripeCustomerID string
	Quantity         string
	FactSetSHA256    string
	Status           string
	CreatedAt        time.Time
}

// LedgerHead is the latest row for a tenant-hour-meter.
type LedgerHead struct {
	SubmissionID  uuid.UUID
	BatchID       uuid.UUID
	Seq           int
	Status        string
	FactSetSHA256 string
}

type LedgerKey struct {
	Tenant    string
	HourStart time.Time
	Meter     string
}

type Batch struct {
	BatchID         uuid.UUID
	ClusterID       string
	CutoverHour     time.Time
	WindowStart     time.Time
	WindowEnd       time.Time
	DryRun          bool
	ArchiveBucket   string
	ArchiveKey      string
	ArchiveSHA256   string
	SubmissionCount int
	CreatedAt       time.Time
	ArchivedAt      time.Time
	CompletedAt     time.Time
}

// Outcome is a terminal status applied to an archived submission.
type Outcome struct {
	Status        string
	StripeEventID string
	ErrorCode     string
	ErrorMessage  string
	SentAt        time.Time
}

type Store interface {
	TryLock(ctx context.Context) (bool, func(context.Context) error, error)
	LatestLiveCutover(ctx context.Context) (time.Time, bool, error)
	CompletedHours(ctx context.Context, clusterID string, from, to time.Time) ([]time.Time, error)
	AggregateTenantHours(ctx context.Context, clusterID string, hours []time.Time) ([]TenantHour, error)
	LedgerHeads(ctx context.Context, hours []time.Time) (map[LedgerKey]LedgerHead, error)
	CreateBatch(ctx context.Context, batch Batch, submissions []Submission) ([]Submission, error)
	IncompleteBatches(ctx context.Context) ([]Batch, error)
	BatchSubmissions(ctx context.Context, batchID uuid.UUID) ([]Submission, error)
	MarkBatchArchived(ctx context.Context, batchID uuid.UUID, sha256 string, at time.Time) error
	MarkBatchCompleted(ctx context.Context, batchID uuid.UUID, at time.Time) error
	MarkSubmission(ctx context.Context, submissionID uuid.UUID, outcome Outcome) error
}

// Archiver writes one object. Implementations must not retry silently in a
// way that hides a permanent failure.
type Archiver interface {
	Put(ctx context.Context, bucket, key, contentType string, body []byte) error
}

// CustomerResolver maps a capsule tenant to a Stripe customer. The dry-run
// binary uses NoCustomers until the Stripe gateway is wired in.
type CustomerResolver interface {
	// Resolve returns the Stripe customer id, or found=false when the tenant
	// has no customer yet. A lookup failure is an error, never found=false.
	Resolve(ctx context.Context, tenant string) (customerID string, found bool, err error)
}

type NoCustomers struct{}

func (NoCustomers) Resolve(context.Context, string) (string, bool, error) { return "", false, nil }

type Result struct {
	BatchID          uuid.UUID
	ResumedBatches   int
	HoursConsidered  int
	TenantHours      int
	Created          int
	Unchanged        int
	Superseded       int
	DryRun           int
	Unmapped         int
	OldestUnsentHour time.Time
}

type Runner struct {
	Config    Config
	Store     Store
	Archiver  Archiver
	Customers CustomerResolver
	Now       func() time.Time
	NewUUID   func() uuid.UUID
}

func (r Runner) Run(ctx context.Context) (Result, error) {
	if err := r.Config.validate(); err != nil {
		return Result{}, err
	}
	if r.Store == nil || r.Archiver == nil {
		return Result{}, errors.New("store and archiver are required")
	}
	if r.Customers == nil {
		r.Customers = NoCustomers{}
	}
	if r.Now == nil {
		r.Now = time.Now
	}
	if r.NewUUID == nil {
		r.NewUUID = uuid.New
	}

	locked, unlock, err := r.Store.TryLock(ctx)
	if err != nil {
		return Result{}, fmt.Errorf("acquire submitter lock: %w", err)
	}
	if !locked {
		return Result{}, ErrAlreadyRunning
	}
	result, runErr := r.run(ctx)
	if unlockErr := unlock(ctx); unlockErr != nil {
		return result, errors.Join(runErr, fmt.Errorf("release submitter lock: %w", unlockErr))
	}
	return result, runErr
}

var ErrAlreadyRunning = errors.New("another submitter run holds the lock")

func (r Runner) run(ctx context.Context) (Result, error) {
	var result Result

	liveCutover, hasLive, err := r.Store.LatestLiveCutover(ctx)
	if err != nil {
		return result, fmt.Errorf("read live cutover: %w", err)
	}
	if hasLive && r.Config.CutoverHour.Before(liveCutover) {
		return result, fmt.Errorf("configured cutover %s is earlier than the live batch cutover %s", r.Config.CutoverHour.Format(time.RFC3339), liveCutover.Format(time.RFC3339))
	}

	incomplete, err := r.Store.IncompleteBatches(ctx)
	if err != nil {
		return result, fmt.Errorf("list incomplete batches: %w", err)
	}
	for _, batch := range incomplete {
		if err := r.completeBatch(ctx, batch, &result); err != nil {
			return result, fmt.Errorf("resume batch %s: %w", batch.BatchID, err)
		}
		result.ResumedBatches++
	}

	now := r.Now().UTC()
	windowEnd := now.Add(-r.Config.SettlementLag).Truncate(time.Hour)
	windowStart := now.Add(-r.Config.Lookback).Truncate(time.Hour)
	if windowStart.Before(r.Config.CutoverHour) {
		windowStart = r.Config.CutoverHour
	}
	if !windowEnd.After(windowStart) {
		return result, nil
	}

	hours, err := r.Store.CompletedHours(ctx, r.Config.ClusterID, windowStart, windowEnd)
	if err != nil {
		return result, fmt.Errorf("list completed hours: %w", err)
	}
	result.HoursConsidered = len(hours)
	if len(hours) == 0 {
		return result, nil
	}

	tenantHours, err := r.Store.AggregateTenantHours(ctx, r.Config.ClusterID, hours)
	if err != nil {
		return result, fmt.Errorf("aggregate tenant hours: %w", err)
	}
	result.TenantHours = len(tenantHours)
	heads, err := r.Store.LedgerHeads(ctx, hours)
	if err != nil {
		return result, fmt.Errorf("read ledger heads: %w", err)
	}

	batchID := r.NewUUID()
	var submissions []Submission
	var superseded []uuid.UUID
	customers := map[string]customerLookup{}
	for _, tenantHour := range tenantHours {
		for _, meter := range Meters {
			hash, err := FactSetSHA256(meter, tenantHour.Facts)
			if err != nil {
				return result, err
			}
			key := LedgerKey{Tenant: tenantHour.Tenant, HourStart: tenantHour.HourStart, Meter: meter}
			head, exists := heads[key]
			seq := 1
			if exists {
				if head.FactSetSHA256 == hash && (head.Status == StatusSent || head.Status == StatusDryRun || head.Status == StatusArchived) {
					result.Unchanged++
					if head.Status == StatusArchived {
						result.OldestUnsentHour = olderOf(result.OldestUnsentHour, tenantHour.HourStart)
					}
					continue
				}
				if head.Status == StatusArchived {
					superseded = append(superseded, head.SubmissionID)
				}
				seq = head.Seq + 1
			}
			identifier, err := Identifier(meter, tenantHour.Tenant, tenantHour.HourStart, seq)
			if err != nil {
				return result, err
			}
			quantity, err := tenantHour.Quantity(meter)
			if err != nil {
				return result, err
			}
			lookup, cached := customers[tenantHour.Tenant]
			if !cached {
				lookup.id, lookup.found, err = r.Customers.Resolve(ctx, tenantHour.Tenant)
				if err != nil {
					return result, fmt.Errorf("resolve customer for tenant %q: %w", tenantHour.Tenant, err)
				}
				customers[tenantHour.Tenant] = lookup
			}
			submissions = append(submissions, Submission{
				SubmissionID:     r.NewUUID(),
				BatchID:          batchID,
				Tenant:           tenantHour.Tenant,
				HourStart:        tenantHour.HourStart,
				Meter:            meter,
				Seq:              seq,
				Identifier:       identifier,
				StripeCustomerID: lookup.id,
				Quantity:         quantity,
				FactSetSHA256:    hash,
				Status:           StatusArchived,
				CreatedAt:        now,
			})
			result.OldestUnsentHour = olderOf(result.OldestUnsentHour, tenantHour.HourStart)
		}
	}
	if len(submissions) == 0 {
		return result, nil
	}

	for _, submissionID := range superseded {
		if err := r.Store.MarkSubmission(ctx, submissionID, Outcome{Status: StatusFailed, ErrorCode: ErrorCodeSuperseded, ErrorMessage: "facts changed before completion; superseded by a newer seq"}); err != nil {
			return result, fmt.Errorf("supersede in-flight submission %s: %w", submissionID, err)
		}
		result.Superseded++
	}

	batch := Batch{
		BatchID:         batchID,
		ClusterID:       r.Config.ClusterID,
		CutoverHour:     r.Config.CutoverHour,
		WindowStart:     windowStart,
		WindowEnd:       windowEnd,
		DryRun:          r.Config.DryRun,
		ArchiveBucket:   r.Config.ArchiveBucket,
		ArchiveKey:      archiveDataKey(r.Config.ArchivePrefix, batchID),
		SubmissionCount: len(submissions),
		CreatedAt:       now,
	}
	created, err := r.Store.CreateBatch(ctx, batch, submissions)
	if err != nil {
		return result, fmt.Errorf("create batch: %w", err)
	}
	result.BatchID = batchID
	result.Created = len(created)
	if err := r.completeBatch(ctx, batch, &result); err != nil {
		return result, fmt.Errorf("complete batch %s: %w", batchID, err)
	}
	return result, nil
}

type customerLookup struct {
	id    string
	found bool
}

// completeBatch archives a batch (if not yet archived) and applies outcomes to
// every row still in the archived state. It is idempotent so a killed run can
// be resumed by the next one.
func (r Runner) completeBatch(ctx context.Context, batch Batch, result *Result) error {
	rows, err := r.Store.BatchSubmissions(ctx, batch.BatchID)
	if err != nil {
		return fmt.Errorf("read batch submissions: %w", err)
	}
	if batch.ArchivedAt.IsZero() {
		archive, err := BuildArchive(batch, rows, r.Config.ExporterVersion, r.Now().UTC())
		if err != nil {
			return fmt.Errorf("build archive: %w", err)
		}
		if err := r.Archiver.Put(ctx, batch.ArchiveBucket, batch.ArchiveKey, "application/x-ndjson", archive.Data); err != nil {
			return fmt.Errorf("write archive data: %w", err)
		}
		if err := r.Archiver.Put(ctx, batch.ArchiveBucket, archiveManifestKey(batch.ArchiveKey), "application/json", archive.Manifest); err != nil {
			return fmt.Errorf("write archive manifest: %w", err)
		}
		if err := r.Store.MarkBatchArchived(ctx, batch.BatchID, archive.SHA256, r.Now().UTC()); err != nil {
			return fmt.Errorf("mark batch archived: %w", err)
		}
	}
	if !batch.DryRun {
		return errors.New("live submission is not implemented")
	}
	for _, row := range rows {
		if row.Status != StatusArchived {
			continue
		}
		if err := r.Store.MarkSubmission(ctx, row.SubmissionID, Outcome{Status: StatusDryRun}); err != nil {
			return fmt.Errorf("mark submission %s: %w", row.Identifier, err)
		}
		result.DryRun++
	}
	if err := r.Store.MarkBatchCompleted(ctx, batch.BatchID, r.Now().UTC()); err != nil {
		return fmt.Errorf("mark batch completed: %w", err)
	}
	return nil
}

func olderOf(current, candidate time.Time) time.Time {
	if current.IsZero() || candidate.Before(current) {
		return candidate
	}
	return current
}

func archiveDataKey(prefix string, batchID uuid.UUID) string {
	prefix = strings.Trim(prefix, "/")
	key := fmt.Sprintf("schema=%s/%s/batch=%s/submissions.jsonl", SchemaVersion, ArchiveDataset, batchID)
	if prefix == "" {
		return key
	}
	return prefix + "/" + key
}

func archiveManifestKey(dataKey string) string {
	return strings.TrimSuffix(dataKey, "submissions.jsonl") + "manifest.json"
}
