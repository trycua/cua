package submitter

import (
	"strings"
	"testing"
)

func TestStatementsRetainContractParameters(t *testing.T) {
	for name, contract := range map[string]struct {
		statement string
		expect    []string
	}{
		"aggregate": {aggregateTenantHoursStatement, []string{
			"trunc(sum(virtual_cpu_core_seconds) / 3600, 6)",
			"trunc(sum(virtual_memory_byte_seconds) / (1073741824::numeric * 3600), 6)",
			"billing_meter.reservation_hour_current",
			"group by capsule_tenant, hour_start",
		}},
		"completed hours": {completedHoursStatement, []string{"billing_meter.reservation_hour_collection_current", "cluster_id = $1"}},
		"ledger heads":    {ledgerHeadsStatement, []string{"distinct on (capsule_tenant, hour_start, meter)", "seq desc"}},
		"next seq":        {nextSeqStatement, []string{"coalesce(max(seq), 0) + 1"}},
		"mark submission": {markSubmissionStatement, []string{"where submission_id = $1 and status = 'archived'"}},
		"mark archived":   {markBatchArchivedStatement, []string{"archived_at is null"}},
		"mark completed":  {markBatchCompletedStatement, []string{"completed_at is null"}},
		"live cutover":    {latestLiveCutoverStatement, []string{"dry_run = false"}},
	} {
		for _, expected := range contract.expect {
			if !strings.Contains(contract.statement, expected) {
				t.Errorf("%s statement is missing %q", name, expected)
			}
		}
	}
}

func TestNewPostgresStoreRequiresSubmitterRole(t *testing.T) {
	if _, err := NewPostgresStore(t.Context(), "postgres://cyclops_meter_writer:pw@localhost/cyclops"); err == nil || !strings.Contains(err.Error(), submitterRole) {
		t.Fatalf("expected role validation error, got %v", err)
	}
}
