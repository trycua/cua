package submitter

import (
	"context"
	"encoding/json"
	"errors"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
)

type fakeStore struct {
	locked       bool
	liveCutover  *time.Time
	hours        []time.Time
	tenantHours  []TenantHour
	heads        map[LedgerKey]LedgerHead
	batches      map[uuid.UUID]*Batch
	submissions  map[uuid.UUID]*Submission
	failCreate   error
	marks        []Outcome
	createdOrder []uuid.UUID
}

func newFakeStore() *fakeStore {
	return &fakeStore{
		heads:       map[LedgerKey]LedgerHead{},
		batches:     map[uuid.UUID]*Batch{},
		submissions: map[uuid.UUID]*Submission{},
	}
}

func (s *fakeStore) TryLock(context.Context) (bool, func(context.Context) error, error) {
	if s.locked {
		return false, func(context.Context) error { return nil }, nil
	}
	s.locked = true
	return true, func(context.Context) error { s.locked = false; return nil }, nil
}

func (s *fakeStore) LatestLiveCutover(context.Context) (time.Time, bool, error) {
	if s.liveCutover == nil {
		return time.Time{}, false, nil
	}
	return *s.liveCutover, true, nil
}

func (s *fakeStore) CompletedHours(_ context.Context, _ string, from, to time.Time) ([]time.Time, error) {
	var hours []time.Time
	for _, hour := range s.hours {
		if !hour.Before(from) && hour.Before(to) {
			hours = append(hours, hour)
		}
	}
	return hours, nil
}

func (s *fakeStore) AggregateTenantHours(_ context.Context, _ string, hours []time.Time) ([]TenantHour, error) {
	var result []TenantHour
	for _, tenantHour := range s.tenantHours {
		for _, hour := range hours {
			if tenantHour.HourStart.Equal(hour) {
				result = append(result, tenantHour)
			}
		}
	}
	return result, nil
}

func (s *fakeStore) LedgerHeads(context.Context, []time.Time) (map[LedgerKey]LedgerHead, error) {
	heads := map[LedgerKey]LedgerHead{}
	for key, head := range s.heads {
		heads[key] = head
	}
	return heads, nil
}

func (s *fakeStore) CreateBatch(_ context.Context, batch Batch, submissions []Submission) ([]Submission, error) {
	if err := s.failCreate; err != nil {
		return nil, err
	}
	stored := batch
	s.batches[batch.BatchID] = &stored
	for _, submission := range submissions {
		row := submission
		s.submissions[submission.SubmissionID] = &row
		s.createdOrder = append(s.createdOrder, submission.SubmissionID)
		s.heads[LedgerKey{Tenant: row.Tenant, HourStart: row.HourStart, Meter: row.Meter}] = LedgerHead{
			SubmissionID: row.SubmissionID, BatchID: row.BatchID, Seq: row.Seq, Status: row.Status, FactSetSHA256: row.FactSetSHA256,
		}
	}
	return submissions, nil
}

func (s *fakeStore) IncompleteBatches(context.Context) ([]Batch, error) {
	var batches []Batch
	for _, batch := range s.batches {
		if batch.CompletedAt.IsZero() {
			batches = append(batches, *batch)
		}
	}
	return batches, nil
}

func (s *fakeStore) BatchSubmissions(_ context.Context, batchID uuid.UUID) ([]Submission, error) {
	var rows []Submission
	for _, id := range s.createdOrder {
		if row := s.submissions[id]; row.BatchID == batchID {
			rows = append(rows, *row)
		}
	}
	return rows, nil
}

func (s *fakeStore) MarkBatchArchived(_ context.Context, batchID uuid.UUID, sha string, at time.Time) error {
	batch, ok := s.batches[batchID]
	if !ok || !batch.ArchivedAt.IsZero() {
		return errors.New("batch not in unarchived state")
	}
	batch.ArchiveSHA256 = sha
	batch.ArchivedAt = at
	return nil
}

func (s *fakeStore) MarkBatchCompleted(_ context.Context, batchID uuid.UUID, at time.Time) error {
	batch, ok := s.batches[batchID]
	if !ok || !batch.CompletedAt.IsZero() {
		return errors.New("batch not in incomplete state")
	}
	batch.CompletedAt = at
	return nil
}

func (s *fakeStore) MarkSubmission(_ context.Context, submissionID uuid.UUID, outcome Outcome) error {
	row, ok := s.submissions[submissionID]
	if !ok || row.Status != StatusArchived {
		return errors.New("submission not in archived state")
	}
	row.Status = outcome.Status
	s.marks = append(s.marks, outcome)
	key := LedgerKey{Tenant: row.Tenant, HourStart: row.HourStart, Meter: row.Meter}
	head := s.heads[key]
	head.Status = outcome.Status
	s.heads[key] = head
	return nil
}

type fakeArchiver struct {
	objects map[string][]byte
	order   []string
	fail    error
}

func newFakeArchiver() *fakeArchiver { return &fakeArchiver{objects: map[string][]byte{}} }

func (a *fakeArchiver) Put(_ context.Context, bucket, key, _ string, body []byte) error {
	if err := a.fail; err != nil {
		return err
	}
	a.objects[bucket+"/"+key] = body
	a.order = append(a.order, key)
	return nil
}

var (
	cutover = time.Date(2026, 9, 15, 0, 0, 0, 0, time.UTC)
	h0      = time.Date(2026, 9, 15, 10, 0, 0, 0, time.UTC)
	h1      = h0.Add(time.Hour)
	h2      = h0.Add(2 * time.Hour)
	now     = time.Date(2026, 9, 15, 15, 20, 0, 0, time.UTC)
)

// workedExample is the CUA-1150 example: 4 vCPU/8 GiB for 1h, 2 vCPU/4 GiB
// for 30 min, 8 vCPU/32 GiB for 2.5h, all starting at h0.
func workedExample() []TenantHour {
	gib := func(n int64) string { return itoa(n * 1073741824) }
	return []TenantHour{
		{Tenant: "user-acme", HourStart: h0, VCPUHours: "13.000000", GiBHours: "42.000000", Facts: []FactRef{
			{FactID: "a-h0", Revision: 1, CPUCoreSeconds: "14400.000000", MemoryByteSeconds: gib(8 * 3600)},
			{FactID: "b-h0", Revision: 1, CPUCoreSeconds: "3600.000000", MemoryByteSeconds: gib(4 * 1800)},
			{FactID: "c-h0", Revision: 1, CPUCoreSeconds: "28800.000000", MemoryByteSeconds: gib(32 * 3600)},
		}},
		{Tenant: "user-acme", HourStart: h1, VCPUHours: "8.000000", GiBHours: "32.000000", Facts: []FactRef{
			{FactID: "c-h1", Revision: 1, CPUCoreSeconds: "28800.000000", MemoryByteSeconds: gib(32 * 3600)},
		}},
		{Tenant: "user-acme", HourStart: h2, VCPUHours: "4.000000", GiBHours: "16.000000", Facts: []FactRef{
			{FactID: "c-h2", Revision: 1, CPUCoreSeconds: "14400.000000", MemoryByteSeconds: gib(32 * 1800)},
		}},
	}
}

func itoa(n int64) string { return strconv.FormatInt(n, 10) }

func testConfig() Config {
	return Config{
		ClusterID:       "kopf-k3s",
		CutoverHour:     cutover,
		SettlementLag:   2 * time.Hour,
		Lookback:        34 * 24 * time.Hour,
		DryRun:          true,
		ArchiveBucket:   "bucket",
		ArchivePrefix:   "",
		ExporterVersion: "test",
	}
}

func newRunner(store *fakeStore, archiver *fakeArchiver) Runner {
	counter := 0
	return Runner{
		Config:   testConfig(),
		Store:    store,
		Archiver: archiver,
		Now:      func() time.Time { return now },
		NewUUID: func() uuid.UUID {
			counter++
			return uuid.MustParse("00000000-0000-0000-0000-" + strings.Repeat("0", 12-len(itoa(int64(counter)))) + itoa(int64(counter)))
		},
	}
}

func TestRunCreatesOneSubmissionPerTenantHourMeterAndArchivesBeforeMarking(t *testing.T) {
	store := newFakeStore()
	store.hours = []time.Time{h0, h1, h2}
	store.tenantHours = workedExample()
	archiver := newFakeArchiver()

	result, err := newRunner(store, archiver).Run(context.Background())
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if result.Created != 6 || result.DryRun != 6 || result.Unchanged != 0 {
		t.Fatalf("result = %+v, want 6 created and 6 dry_run", result)
	}
	if len(archiver.order) != 2 || !strings.HasSuffix(archiver.order[0], "submissions.jsonl") || !strings.HasSuffix(archiver.order[1], "manifest.json") {
		t.Fatalf("archive order = %v, want data then manifest", archiver.order)
	}
	batch := store.batches[result.BatchID]
	if batch.ArchivedAt.IsZero() || batch.CompletedAt.IsZero() || batch.ArchiveSHA256 == "" {
		t.Fatalf("batch not archived and completed: %+v", *batch)
	}
	if batch.ArchiveKey != "schema=v1/reservation-usage/batch="+result.BatchID.String()+"/submissions.jsonl" {
		t.Fatalf("archive key = %q", batch.ArchiveKey)
	}
	var totals = map[string]string{}
	for _, row := range store.submissions {
		if row.Status != StatusDryRun {
			t.Fatalf("submission %s status = %s", row.Identifier, row.Status)
		}
		if row.Seq != 1 {
			t.Fatalf("submission %s seq = %d, want 1", row.Identifier, row.Seq)
		}
		totals[row.Meter+"@"+row.HourStart.Format(time.RFC3339)] = row.Quantity
	}
	if totals["cua_vcpu_hours@2026-09-15T10:00:00Z"] != "13.000000" || totals["cua_gib_hours@2026-09-15T10:00:00Z"] != "42.000000" {
		t.Fatalf("worked example H0 quantities = %v", totals)
	}
	data := archiver.objects["bucket/"+batch.ArchiveKey]
	if strings.Count(string(data), "\n") != 6 {
		t.Fatalf("archive rows = %d, want 6", strings.Count(string(data), "\n"))
	}
	var manifest archiveManifest
	if err := json.Unmarshal(archiver.objects["bucket/"+archiveManifestKey(batch.ArchiveKey)], &manifest); err != nil {
		t.Fatalf("decode manifest: %v", err)
	}
	if manifest.RowCount != 6 || manifest.DataSHA256 != batch.ArchiveSHA256 || !manifest.DryRun {
		t.Fatalf("manifest = %+v", manifest)
	}
}

func TestRunIsNoOpWhenFactsUnchanged(t *testing.T) {
	store := newFakeStore()
	store.hours = []time.Time{h0, h1, h2}
	store.tenantHours = workedExample()
	archiver := newFakeArchiver()
	runner := newRunner(store, archiver)
	if _, err := runner.Run(context.Background()); err != nil {
		t.Fatalf("first run: %v", err)
	}
	result, err := runner.Run(context.Background())
	if err != nil {
		t.Fatalf("second run: %v", err)
	}
	if result.Created != 0 || result.Unchanged != 6 || len(store.batches) != 1 {
		t.Fatalf("second run result = %+v, batches = %d", result, len(store.batches))
	}
}

func TestRunRevisionCreatesNextSeqForAffectedMeterOnly(t *testing.T) {
	store := newFakeStore()
	store.hours = []time.Time{h0}
	store.tenantHours = workedExample()[:1]
	archiver := newFakeArchiver()
	runner := newRunner(store, archiver)
	if _, err := runner.Run(context.Background()); err != nil {
		t.Fatalf("first run: %v", err)
	}
	// Revision 2 of fact c-h0 changes only the CPU seconds.
	store.tenantHours[0].Facts[2] = FactRef{FactID: "c-h0", Revision: 2, CPUCoreSeconds: "28000.000000", MemoryByteSeconds: store.tenantHours[0].Facts[2].MemoryByteSeconds}
	store.tenantHours[0].VCPUHours = "12.777777"

	result, err := runner.Run(context.Background())
	if err != nil {
		t.Fatalf("second run: %v", err)
	}
	if result.Created != 1 || result.Unchanged != 1 {
		t.Fatalf("result = %+v, want one new CPU submission and one unchanged memory submission", result)
	}
	var found bool
	for _, row := range store.submissions {
		if row.Meter == MeterVCPUHours && row.Seq == 2 {
			found = true
			if row.Identifier != "cua_vcpu_hours:user-acme:2026-09-15T10:00:00Z:r2" || row.Quantity != "12.777777" {
				t.Fatalf("seq 2 row = %+v", *row)
			}
		}
	}
	if !found {
		t.Fatal("expected a seq 2 CPU submission")
	}
}

func TestRunResumesBatchLeftUnarchivedByKilledRun(t *testing.T) {
	store := newFakeStore()
	store.hours = []time.Time{h0}
	store.tenantHours = workedExample()[:1]
	failing := newFakeArchiver()
	failing.fail = errors.New("s3 unavailable")
	runner := newRunner(store, failing)
	if _, err := runner.Run(context.Background()); err == nil || !strings.Contains(err.Error(), "s3 unavailable") {
		t.Fatalf("expected archive failure, got %v", err)
	}
	if len(store.batches) != 1 {
		t.Fatalf("batches = %d, want 1 in-flight", len(store.batches))
	}
	for _, row := range store.submissions {
		if row.Status != StatusArchived {
			t.Fatalf("row %s status = %s, want archived after failed archive", row.Identifier, row.Status)
		}
	}

	healthy := newFakeArchiver()
	runner.Archiver = healthy
	result, err := runner.Run(context.Background())
	if err != nil {
		t.Fatalf("resume run: %v", err)
	}
	if result.ResumedBatches != 1 || result.Created != 0 || result.DryRun != 2 || len(store.batches) != 1 {
		t.Fatalf("resume result = %+v, batches = %d", result, len(store.batches))
	}
	for _, row := range store.submissions {
		if row.Status != StatusDryRun || row.Seq != 1 {
			t.Fatalf("row %s after resume = status %s seq %d", row.Identifier, row.Status, row.Seq)
		}
	}
}

func TestRunRefusesCutoverEarlierThanLiveBatch(t *testing.T) {
	store := newFakeStore()
	live := cutover.Add(24 * time.Hour)
	store.liveCutover = &live
	_, err := newRunner(store, newFakeArchiver()).Run(context.Background())
	if err == nil || !strings.Contains(err.Error(), "earlier than the live batch cutover") {
		t.Fatalf("expected cutover refusal, got %v", err)
	}
}

func TestRunSkipsHoursBeforeCutoverAndInsideSettlementLag(t *testing.T) {
	store := newFakeStore()
	early := cutover.Add(-time.Hour)
	recent := now.Truncate(time.Hour).Add(-time.Hour) // hour_end is inside the 2h lag
	store.hours = []time.Time{early, h0, recent}
	store.tenantHours = append(workedExample()[:1],
		TenantHour{Tenant: "user-acme", HourStart: early, VCPUHours: "1.000000", GiBHours: "1.000000", Facts: []FactRef{{FactID: "e", Revision: 1, CPUCoreSeconds: "3600", MemoryByteSeconds: "3865470566400"}}},
		TenantHour{Tenant: "user-acme", HourStart: recent, VCPUHours: "1.000000", GiBHours: "1.000000", Facts: []FactRef{{FactID: "r", Revision: 1, CPUCoreSeconds: "3600", MemoryByteSeconds: "3865470566400"}}},
	)
	result, err := newRunner(store, newFakeArchiver()).Run(context.Background())
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if result.HoursConsidered != 1 || result.Created != 2 {
		t.Fatalf("result = %+v, want only h0 considered", result)
	}
}

func TestRunWhenLockHeldReturnsErrAlreadyRunning(t *testing.T) {
	store := newFakeStore()
	store.locked = true
	_, err := newRunner(store, newFakeArchiver()).Run(context.Background())
	if !errors.Is(err, ErrAlreadyRunning) {
		t.Fatalf("err = %v, want ErrAlreadyRunning", err)
	}
}

func TestConfigRejectsLiveMode(t *testing.T) {
	config := testConfig()
	config.DryRun = false
	if err := config.validate(); err == nil {
		t.Fatal("expected live mode to be rejected until implemented")
	}
}

func TestFactSetSHA256IsOrderIndependentAndMeterSpecific(t *testing.T) {
	facts := workedExample()[0].Facts
	reversed := []FactRef{facts[2], facts[1], facts[0]}
	a, err := FactSetSHA256(MeterVCPUHours, facts)
	if err != nil {
		t.Fatal(err)
	}
	b, err := FactSetSHA256(MeterVCPUHours, reversed)
	if err != nil {
		t.Fatal(err)
	}
	if a != b {
		t.Fatal("hash depends on fact order")
	}
	memory, err := FactSetSHA256(MeterGiBHours, facts)
	if err != nil {
		t.Fatal(err)
	}
	if memory == a {
		t.Fatal("cpu and memory hashes must differ")
	}
	if _, err := FactSetSHA256("cua_unknown", facts); err == nil {
		t.Fatal("unknown meter must be rejected")
	}
}

func TestIdentifierFormatAndLength(t *testing.T) {
	identifier, err := Identifier(MeterGiBHours, "user-acme", h0, 3)
	if err != nil {
		t.Fatal(err)
	}
	if identifier != "cua_gib_hours:user-acme:2026-09-15T10:00:00Z:r3" {
		t.Fatalf("identifier = %q", identifier)
	}
	if _, err := Identifier(MeterGiBHours, strings.Repeat("t", 80), h0, 1); err == nil {
		t.Fatal("expected identifier length rejection")
	}
	if _, err := Identifier(MeterGiBHours, "user-acme", h0, 0); err == nil {
		t.Fatal("expected seq validation")
	}
}

func TestBuildArchiveIsDeterministic(t *testing.T) {
	batchID := uuid.MustParse("11111111-1111-1111-1111-111111111111")
	batch := Batch{BatchID: batchID, ClusterID: "kopf-k3s", CutoverHour: cutover, WindowStart: h0, WindowEnd: h2, DryRun: true, ArchiveKey: archiveDataKey("", batchID)}
	rows := []Submission{
		{SubmissionID: uuid.MustParse("22222222-2222-2222-2222-222222222222"), BatchID: batchID, Tenant: "user-acme", HourStart: h0, Meter: MeterGiBHours, Seq: 1, Identifier: "cua_gib_hours:user-acme:2026-09-15T10:00:00Z:r1", Quantity: "42.000000", FactSetSHA256: strings.Repeat("b", 64)},
		{SubmissionID: uuid.MustParse("33333333-3333-3333-3333-333333333333"), BatchID: batchID, Tenant: "user-acme", HourStart: h0, Meter: MeterVCPUHours, Seq: 1, Identifier: "cua_vcpu_hours:user-acme:2026-09-15T10:00:00Z:r1", Quantity: "13.000000", FactSetSHA256: strings.Repeat("a", 64)},
	}
	first, err := BuildArchive(batch, rows, "test", now)
	if err != nil {
		t.Fatal(err)
	}
	second, err := BuildArchive(batch, []Submission{rows[1], rows[0]}, "test", now)
	if err != nil {
		t.Fatal(err)
	}
	if first.SHA256 != second.SHA256 || string(first.Data) != string(second.Data) {
		t.Fatal("archive depends on input order")
	}
	if !strings.HasPrefix(string(first.Data), `{"submission_id":"22222222`) {
		t.Fatalf("archive not sorted by identifier: %s", first.Data)
	}
	other := rows[0]
	other.BatchID = uuid.MustParse("44444444-4444-4444-4444-444444444444")
	if _, err := BuildArchive(batch, []Submission{other}, "test", now); err == nil {
		t.Fatal("expected foreign batch row rejection")
	}
}
