package usage

import (
	"context"
	"testing"
	"time"
)

func TestNewPostgresEventStoreRequiresUsageReader(t *testing.T) {
	t.Parallel()
	if _, err := NewPostgresEventStore(context.Background(), "postgres://wrong:pw@db/cyclops"); err == nil {
		t.Fatal("expected role validation error")
	}
}

func TestUsageSandboxEventsQueryIsFixed(t *testing.T) {
	t.Parallel()
	const want = "select event_id, namespace, sandbox_name, sandbox_uid, pool_name,\n       runtime, vm_name, event_type, observed_at\nfrom k8s_reporting.usage_sandbox_events($1, $2, $3)"
	if usageSandboxEventsQuery != want {
		t.Fatalf("unexpected query: %q", usageSandboxEventsQuery)
	}
}

func TestReservationQueriesAreFixed(t *testing.T) {
	t.Parallel()
	const wantStatus = "select data_as_of, complete\nfrom k8s_reporting.reservation_meter_status($1, $2, $3)"
	if reservationStatusQuery != wantStatus {
		t.Fatalf("unexpected status query: %q", reservationStatusQuery)
	}
	const wantFacts = "select namespace, sandbox_uid, sandbox_name, pool_name, runtime,\n       hour_start, hour_end, virtual_cpu_core_seconds, virtual_memory_byte_seconds,\n       ready_seconds, covered_seconds\nfrom k8s_reporting.reservation_hour_facts($1, $2, $3)"
	if reservationFactsQuery != wantFacts {
		t.Fatalf("unexpected facts query: %q", reservationFactsQuery)
	}
}

func TestNewPostgresEventStoreAcceptsUsageReaderAndCloses(t *testing.T) {
	t.Parallel()
	store, err := NewPostgresEventStore(context.Background(), "postgres://cyclops_usage_reader:pw@127.0.0.1:1/cyclops")
	if err != nil {
		t.Fatal(err)
	}
	store.Close()
}

func TestValidateSandboxEventRejectsUnnormalizedOrUnsafeNamespace(t *testing.T) {
	t.Parallel()
	valid := SandboxEvent{EventID: mustUUID("00000000-0000-0000-0000-000000000050"), Namespace: "ns-a", SandboxName: "sandbox", SandboxUID: "uid", PoolName: "pool", Runtime: "kubevirt", VMName: "vm", EventType: "Added", ObservedAt: time.Now().UTC()}
	for name, mutate := range map[string]func(*SandboxEvent){
		"whitespace": func(event *SandboxEvent) { event.PoolName = " pool" },
		"namespace":  func(event *SandboxEvent) { event.Namespace = "ns-a\"" },
		"pool_colon": func(event *SandboxEvent) { event.PoolName = "pool:a" },
		"pool_case":  func(event *SandboxEvent) { event.PoolName = "Pool" },
		"pool_length": func(event *SandboxEvent) {
			event.PoolName = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
		},
	} {
		t.Run(name, func(t *testing.T) {
			event := valid
			mutate(&event)
			if err := validateSandboxEvent(event); err == nil {
				t.Fatal("expected validation error")
			}
		})
	}
}
