package usage

import (
	"context"
	"errors"
	"github.com/google/uuid"
	"strings"
	"testing"
	"time"
)

type fakeEventStore struct {
	events []SandboxEvent
	err    error
	calls  int
	tenant string
	start  time.Time
	end    time.Time
}

func (s *fakeEventStore) Events(_ context.Context, tenant string, start, end time.Time) ([]SandboxEvent, error) {
	s.calls++
	s.tenant, s.start, s.end = tenant, start, end
	return s.events, s.err
}

type fakeAllocationClient struct {
	allocations []Allocation
	asOf        time.Time
	partial     bool
	err         error
	calls       int
	start       time.Time
	end         time.Time
	step        time.Duration
	namespaces  []string
}

func (c *fakeAllocationClient) Allocations(_ context.Context, start, end time.Time, step time.Duration, namespaces []string) ([]Allocation, time.Time, bool, error) {
	c.calls++
	c.start, c.end, c.step = start, end, step
	c.namespaces = append([]string(nil), namespaces...)
	return c.allocations, c.asOf, c.partial, c.err
}

func TestProviderOverviewAggregatesTenantLauncherUsage(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	events := &fakeEventStore{events: []SandboxEvent{{
		EventID: mustUUID("00000000-0000-0000-0000-000000000001"), Namespace: "ns-a", SandboxName: "sandbox-a", SandboxUID: "uid-a",
		PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm-a", EventType: "Added", ObservedAt: cutoff.Add(-25 * time.Hour),
	}}}
	allocations := &fakeAllocationClient{asOf: cutoff, allocations: []Allocation{{
		Start: cutoff.Add(-time.Hour), End: cutoff, Namespace: "ns-a", Pod: "virt-launcher-vm-a-abc",
		Minutes: 60, CPUUsageAverage: 2, CPURequestAverage: 4,
		RAMUsageAverageBytes: 3 * gibibyte, RAMRequestAverageBytes: 6 * gibibyte, CostUSD: 1.25,
	}}}
	provider := NewProvider(events, allocations, func() time.Time { return cutoff })

	got, err := provider.Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H})
	if err != nil {
		t.Fatal(err)
	}
	if events.tenant != "user-alice" || !events.start.Equal(cutoff.Add(-24*time.Hour)) || !events.end.Equal(cutoff) {
		t.Fatalf("event query = tenant %q [%s, %s)", events.tenant, events.start, events.end)
	}
	if allocations.calls != 1 || allocations.step != time.Hour || len(allocations.namespaces) != 1 || allocations.namespaces[0] != "ns-a" {
		t.Fatalf("allocation request = calls %d step %s namespaces %#v", allocations.calls, allocations.step, allocations.namespaces)
	}
	if !got.DataAsOf.Equal(cutoff) || got.Partial || len(got.Pools) != 1 {
		t.Fatalf("overview = %#v", got)
	}
	pool := got.Pools[0]
	if pool.ID != "ns-a:pool-a" || pool.Name != "pool-a" || pool.CPU.Consumed != 2 || pool.CPU.Provisioned != 4 || pool.Memory.Consumed != 3 || pool.Memory.Provisioned != 6 || pool.CostUSD != 1.25 {
		t.Fatalf("pool = %#v", pool)
	}
}

func TestProviderOverviewEmptyTenantSkipsOpenCost(t *testing.T) {
	t.Parallel()
	events := &fakeEventStore{}
	allocations := &fakeAllocationClient{}
	provider := NewProvider(events, allocations, func() time.Time { return time.Date(2026, 8, 19, 10, 22, 0, 0, time.UTC) })

	got, err := provider.Overview(context.Background(), Query{Subject: "nobody", Timeframe: Timeframe7D})
	if err != nil {
		t.Fatal(err)
	}
	if allocations.calls != 0 || got.Partial || len(got.Pools) != 0 || !got.DataAsOf.Equal(time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)) {
		t.Fatalf("overview = %#v, allocation calls = %d", got, allocations.calls)
	}
}

func TestProviderSourceErrorsNeverReturnPartialData(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	for name, provider := range map[string]*provider{
		"events":      NewProvider(&fakeEventStore{err: errors.New("db unavailable")}, &fakeAllocationClient{}, func() time.Time { return cutoff }),
		"allocations": NewProvider(&fakeEventStore{events: []SandboxEvent{{EventID: mustUUID("00000000-0000-0000-0000-000000000002"), Namespace: "ns-a", SandboxName: "sandbox", SandboxUID: "uid", PoolName: "pool", Runtime: "kubevirt", VMName: "vm", EventType: "Added", ObservedAt: cutoff.Add(-48 * time.Hour)}}}, &fakeAllocationClient{err: errors.New("opencost unavailable")}, func() time.Time { return cutoff }),
	} {
		t.Run(name, func(t *testing.T) {
			got, err := provider.Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H})
			if err == nil || got.Partial || len(got.Pools) != 0 {
				t.Fatalf("overview = %#v, err = %v", got, err)
			}
		})
	}
}

func mustUUID(raw string) uuid.UUID {
	return uuid.MustParse(raw)
}

func TestProviderOverviewCoverageAndMatchingExclusionsArePartial(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	base := func(vmName, uid, pool string, eventType string, observedAt time.Time) SandboxEvent {
		return SandboxEvent{EventID: mustUUID(map[string]string{"uid-a": "00000000-0000-0000-0000-000000000010", "uid-b": "00000000-0000-0000-0000-000000000011"}[uid]), Namespace: "ns-a", SandboxName: uid, SandboxUID: uid, PoolName: pool, Runtime: "kubevirt", VMName: vmName, EventType: eventType, ObservedAt: observedAt}
	}
	allocation := Allocation{Start: cutoff.Add(-time.Hour), End: cutoff, Namespace: "ns-a", Pod: "virt-launcher-vm-a-suffix", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 2, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: 2 * gibibyte}
	for name, events := range map[string][]SandboxEvent{
		"missing_projector_baseline": {base("vm-a", "uid-a", "pool-a", "Modified", cutoff.Add(-time.Hour))},
		"ambiguous_launcher": {
			base("vm-a", "uid-a", "pool-a", "Added", cutoff.Add(-25*time.Hour)),
			base("vm-a", "uid-b", "pool-b", "Added", cutoff.Add(-25*time.Hour)),
		},
	} {
		t.Run(name, func(t *testing.T) {
			provider := NewProvider(&fakeEventStore{events: events}, &fakeAllocationClient{asOf: cutoff, allocations: []Allocation{allocation}}, func() time.Time { return cutoff })
			got, err := provider.Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H})
			if err != nil || !got.Partial {
				t.Fatalf("overview=%#v err=%v", got, err)
			}
		})
	}
}

func TestProviderOverviewUsesLongestKubeVirtVMName(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	events := []SandboxEvent{
		{EventID: mustUUID("00000000-0000-0000-0000-000000000020"), Namespace: "ns-a", SandboxName: "short", SandboxUID: "short", PoolName: "short-pool", Runtime: "kubevirt", VMName: "vm", EventType: "Added", ObservedAt: cutoff.Add(-25 * time.Hour)},
		{EventID: mustUUID("00000000-0000-0000-0000-000000000021"), Namespace: "ns-a", SandboxName: "long", SandboxUID: "long", PoolName: "long-pool", Runtime: "kubevirt", VMName: "vm-a", EventType: "Added", ObservedAt: cutoff.Add(-25 * time.Hour)},
	}
	allocation := Allocation{Start: cutoff.Add(-time.Hour), End: cutoff, Namespace: "ns-a", Pod: "virt-launcher-vm-a-nonempty", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 1, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: gibibyte}
	got, err := NewProvider(&fakeEventStore{events: events}, &fakeAllocationClient{asOf: cutoff, allocations: []Allocation{allocation}}, func() time.Time { return cutoff }).Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H})
	if err != nil || got.Partial || len(got.Pools) != 1 || got.Pools[0].ID != "ns-a:long-pool" {
		t.Fatalf("overview=%#v err=%v", got, err)
	}
}

func TestProviderPoolDetailBuildsDeterministicBuckets(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	events := []SandboxEvent{{EventID: mustUUID("00000000-0000-0000-0000-000000000030"), Namespace: "ns-a", SandboxName: "sandbox", SandboxUID: "uid", PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm", EventType: "Added", ObservedAt: cutoff.Add(-31 * 24 * time.Hour)}}
	allocation := Allocation{Start: cutoff.Add(-24 * time.Hour), End: cutoff, Namespace: "ns-a", Pod: "virt-launcher-vm-a", Minutes: 24 * 60, CPUUsageAverage: 1, CPURequestAverage: 2, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: 2 * gibibyte}
	provider := NewProvider(&fakeEventStore{events: events}, &fakeAllocationClient{asOf: cutoff, allocations: []Allocation{allocation}}, func() time.Time { return cutoff })
	got, err := provider.PoolDetail(context.Background(), PoolQuery{Query: Query{Subject: "alice", Timeframe: Timeframe30D}, PoolID: "ns-a:pool-a", Interval: IntervalDay})
	if err != nil || len(got.Buckets) != 30 || !got.Buckets[0].Start.Equal(cutoff.Add(-30*24*time.Hour)) || !got.Buckets[29].End.Equal(cutoff) {
		t.Fatalf("detail=%#v err=%v", got, err)
	}
	last := got.Buckets[29]
	if last.CPUConsumed != 24 || last.CPUProvisioned != 48 || last.MemoryConsumed != 24 || last.MemoryProvisioned != 48 {
		t.Fatalf("last bucket=%#v", last)
	}
}

func TestProviderMarksPartiallyUncoveredAllocationPartial(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	event := SandboxEvent{EventID: mustUUID("00000000-0000-0000-0000-000000000040"), Namespace: "ns-a", SandboxName: "sandbox", SandboxUID: "uid", PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm", EventType: "Added", ObservedAt: cutoff.Add(-30 * time.Minute)}
	allocation := Allocation{Start: cutoff.Add(-time.Hour), End: cutoff, Namespace: "ns-a", Pod: "virt-launcher-vm-a", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 1, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: gibibyte, CostUSD: 2}
	got, err := NewProvider(&fakeEventStore{events: []SandboxEvent{event}}, &fakeAllocationClient{asOf: cutoff, allocations: []Allocation{allocation}}, func() time.Time { return cutoff }).Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H})
	if err != nil || !got.Partial || len(got.Pools) != 1 || got.Pools[0].CPU.Consumed != 0.5 || got.Pools[0].CostUSD != 1 {
		t.Fatalf("overview=%#v err=%v", got, err)
	}
}

func TestProviderPropagatesAllocationCoverageCutoff(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	asOf := cutoff.Add(-time.Hour)
	event := SandboxEvent{EventID: mustUUID("00000000-0000-0000-0000-000000000060"), Namespace: "ns-a", SandboxName: "sandbox", SandboxUID: "uid", PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm", EventType: "Added", ObservedAt: cutoff.Add(-48 * time.Hour)}
	allocation := Allocation{Start: asOf.Add(-time.Hour), End: asOf, Namespace: "ns-a", Pod: "virt-launcher-vm-a", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 2, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: 2 * gibibyte}
	got, err := NewProvider(&fakeEventStore{events: []SandboxEvent{event}}, &fakeAllocationClient{asOf: asOf, allocations: []Allocation{allocation}}, func() time.Time { return cutoff }).Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H})
	if err != nil || !got.Partial || !got.DataAsOf.Equal(asOf) || len(got.Pools) != 1 {
		t.Fatalf("overview=%#v err=%v", got, err)
	}
}

func TestProviderPoolDetailRejectsPoolOutsideSubjectOverview(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	event := SandboxEvent{EventID: mustUUID("00000000-0000-0000-0000-000000000061"), Namespace: "ns-a", SandboxName: "sandbox", SandboxUID: "uid", PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm", EventType: "Added", ObservedAt: cutoff.Add(-48 * time.Hour)}
	allocation := Allocation{Start: cutoff.Add(-time.Hour), End: cutoff, Namespace: "ns-a", Pod: "virt-launcher-vm-a", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 1, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: gibibyte}
	provider := NewProvider(&fakeEventStore{events: []SandboxEvent{event}}, &fakeAllocationClient{asOf: cutoff, allocations: []Allocation{allocation}}, func() time.Time { return cutoff })
	_, err := provider.PoolDetail(context.Background(), PoolQuery{Query: Query{Subject: "alice", Timeframe: Timeframe24H}, PoolID: "ns-a:pool-missing", Interval: IntervalHour})
	if !errors.Is(err, ErrPoolNotFound) {
		t.Fatalf("err=%v, want ErrPoolNotFound", err)
	}
}

func TestProviderOmitsZeroUsagePools(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	event := SandboxEvent{EventID: mustUUID("00000000-0000-0000-0000-000000000062"), Namespace: "ns-a", SandboxName: "sandbox", SandboxUID: "uid", PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm", EventType: "Added", ObservedAt: cutoff.Add(-48 * time.Hour)}
	allocation := Allocation{Start: cutoff.Add(-time.Hour), End: cutoff, Namespace: "ns-a", Pod: "virt-launcher-vm-a", Minutes: 0, CPUUsageAverage: 0, CPURequestAverage: 0, RAMUsageAverageBytes: 0, RAMRequestAverageBytes: 0}
	got, err := NewProvider(&fakeEventStore{events: []SandboxEvent{event}}, &fakeAllocationClient{asOf: cutoff, allocations: []Allocation{allocation}}, func() time.Time { return cutoff }).Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H})
	if err != nil || len(got.Pools) != 0 {
		t.Fatalf("overview=%#v err=%v", got, err)
	}
}

func TestProviderDisambiguatesDuplicatePoolDisplayNames(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	events := []SandboxEvent{
		{EventID: mustUUID("00000000-0000-0000-0000-000000000063"), Namespace: "ns-a", SandboxName: "sandbox-a", SandboxUID: "uid-a", PoolName: "pool", Runtime: "kubevirt", VMName: "vm-a", EventType: "Added", ObservedAt: cutoff.Add(-48 * time.Hour)},
		{EventID: mustUUID("00000000-0000-0000-0000-000000000064"), Namespace: "ns-b", SandboxName: "sandbox-b", SandboxUID: "uid-b", PoolName: "pool", Runtime: "kubevirt", VMName: "vm-b", EventType: "Added", ObservedAt: cutoff.Add(-48 * time.Hour)},
	}
	allocations := []Allocation{
		{Start: cutoff.Add(-time.Hour), End: cutoff, Namespace: "ns-a", Pod: "virt-launcher-vm-a-x", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 1, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: gibibyte},
		{Start: cutoff.Add(-time.Hour), End: cutoff, Namespace: "ns-b", Pod: "virt-launcher-vm-b-x", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 1, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: gibibyte},
	}
	got, err := NewProvider(&fakeEventStore{events: events}, &fakeAllocationClient{asOf: cutoff, allocations: allocations}, func() time.Time { return cutoff }).Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H})
	if err != nil || len(got.Pools) != 2 || got.Pools[0].Name != "ns-a/pool" || got.Pools[1].Name != "ns-b/pool" {
		t.Fatalf("overview=%#v err=%v", got, err)
	}
}

func TestBuildSegmentsMarksInitialAndImpossibleEventSequencesPartial(t *testing.T) {
	t.Parallel()
	start := time.Date(2026, 8, 19, 9, 0, 0, 0, time.UTC)
	base := func(eventID, eventType string, observedAt time.Time) SandboxEvent {
		return SandboxEvent{EventID: mustUUID(eventID), Namespace: "ns-a", SandboxName: "sandbox", SandboxUID: "uid", PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm", EventType: eventType, ObservedAt: observedAt}
	}
	for name, events := range map[string][]SandboxEvent{
		"initial_added_in_window": {
			base("00000000-0000-0000-0000-000000000065", "Added", start.Add(time.Minute)),
		},
		"modified_after_deleted": {
			base("00000000-0000-0000-0000-000000000066", "Added", start.Add(-time.Hour)),
			base("00000000-0000-0000-0000-000000000067", "Deleted", start.Add(time.Minute)),
			base("00000000-0000-0000-0000-000000000068", "Modified", start.Add(2*time.Minute)),
		},
		"duplicate_added_while_active": {
			base("00000000-0000-0000-0000-000000000069", "Added", start.Add(-time.Hour)),
			base("00000000-0000-0000-0000-000000000070", "Added", start.Add(time.Minute)),
		},
		"equal_timestamp_ambiguity": {
			base("00000000-0000-0000-0000-000000000071", "Added", start.Add(-time.Hour)),
			base("00000000-0000-0000-0000-000000000072", "Modified", start.Add(time.Minute)),
			base("00000000-0000-0000-0000-000000000073", "Deleted", start.Add(time.Minute)),
		},
	} {
		t.Run(name, func(t *testing.T) {
			_, _, partial := buildSegments(events, start, start.Add(time.Hour))
			if !partial {
				t.Fatal("expected partial event coverage")
			}
		})
	}
}

func TestProviderOverviewAttributesPreWindowModifiedBaseline(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	event := SandboxEvent{EventID: mustUUID("00000000-0000-0000-0000-000000000080"), Namespace: "ns-a", SandboxName: "sandbox", SandboxUID: "uid", PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm", EventType: "Modified", ObservedAt: cutoff.Add(-48 * time.Hour)}
	allocation := Allocation{Start: cutoff.Add(-time.Hour), End: cutoff, Namespace: "ns-a", Pod: "virt-launcher-vm-a", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 2, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: 2 * gibibyte}
	got, err := NewProvider(&fakeEventStore{events: []SandboxEvent{event}}, &fakeAllocationClient{asOf: cutoff, allocations: []Allocation{allocation}}, func() time.Time { return cutoff }).Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H})
	if err != nil || got.Partial || len(got.Pools) != 1 || got.Pools[0].CPU.Consumed != 1 || got.Pools[0].CPU.Provisioned != 2 {
		t.Fatalf("overview=%#v err=%v", got, err)
	}
}

func TestProviderExcludesUncertainTimelineRemainder(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	start := cutoff.Add(-24 * time.Hour)
	base := func(eventID, eventType string, observedAt time.Time) SandboxEvent {
		return SandboxEvent{EventID: mustUUID(eventID), Namespace: "ns-a", SandboxName: "sandbox", SandboxUID: "uid", PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm", EventType: eventType, ObservedAt: observedAt}
	}
	before := Allocation{Start: start, End: start.Add(time.Hour), Namespace: "ns-a", Pod: "virt-launcher-vm-a", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 1, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: gibibyte}
	after := Allocation{Start: start.Add(2 * time.Hour), End: start.Add(3 * time.Hour), Namespace: "ns-a", Pod: "virt-launcher-vm-a", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 1, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: gibibyte}
	for name, events := range map[string][]SandboxEvent{
		"duplicate_added": {
			base("00000000-0000-0000-0000-000000000081", "Added", start.Add(-time.Hour)),
			base("00000000-0000-0000-0000-000000000082", "Added", start.Add(time.Hour)),
		},
		"unknown_event": {
			base("00000000-0000-0000-0000-000000000083", "Added", start.Add(-time.Hour)),
			base("00000000-0000-0000-0000-000000000084", "Unexpected", start.Add(time.Hour)),
		},
		"equal_timestamp": {
			base("00000000-0000-0000-0000-000000000085", "Added", start.Add(-time.Hour)),
			base("00000000-0000-0000-0000-000000000086", "Modified", start.Add(time.Hour)),
			base("00000000-0000-0000-0000-000000000087", "Deleted", start.Add(time.Hour)),
		},
		"modified_after_deleted": {
			base("00000000-0000-0000-0000-000000000088", "Added", start.Add(-time.Hour)),
			base("00000000-0000-0000-0000-000000000089", "Deleted", start.Add(time.Hour)),
			base("00000000-0000-0000-0000-000000000090", "Modified", start.Add(2*time.Hour)),
		},
		"deleted_while_inactive": {
			base("00000000-0000-0000-0000-000000000091", "Added", start.Add(-time.Hour)),
			base("00000000-0000-0000-0000-000000000092", "Deleted", start.Add(time.Hour)),
			base("00000000-0000-0000-0000-000000000093", "Deleted", start.Add(2*time.Hour)),
		},
	} {
		t.Run(name, func(t *testing.T) {
			got, err := NewProvider(&fakeEventStore{events: events}, &fakeAllocationClient{asOf: cutoff, allocations: []Allocation{before, after}}, func() time.Time { return cutoff }).Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H})
			if err != nil || !got.Partial || len(got.Pools) != 1 || got.Pools[0].CPU.Consumed != 1 {
				t.Fatalf("overview=%#v err=%v", got, err)
			}
		})
	}
}

func TestProviderPoolDetailPreservesRequestedWindowWhenOpenCostIsStale(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	for name, test := range map[string]struct {
		timeframe Timeframe
		interval  Interval
		asOf      time.Time
		start     time.Time
		count     int
	}{
		"24h": {Timeframe24H, IntervalHour, cutoff.Add(-time.Hour), cutoff.Add(-2 * time.Hour), 24},
		"7d":  {Timeframe7D, IntervalHour, cutoff.Add(-time.Hour), cutoff.Add(-2 * time.Hour), 7 * 24},
		"30d": {Timeframe30D, IntervalDay, cutoff.Add(-24 * time.Hour), cutoff.Add(-48 * time.Hour), 30},
	} {
		t.Run(name, func(t *testing.T) {
			event := SandboxEvent{EventID: mustUUID("00000000-0000-0000-0000-000000000094"), Namespace: "ns-a", SandboxName: "sandbox", SandboxUID: "uid", PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm", EventType: "Added", ObservedAt: cutoff.Add(-31 * 24 * time.Hour)}
			allocation := Allocation{Start: test.start, End: test.asOf, Namespace: "ns-a", Pod: "virt-launcher-vm-a", Minutes: test.asOf.Sub(test.start).Minutes(), CPUUsageAverage: 1, CPURequestAverage: 1, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: gibibyte}
			client := &fakeAllocationClient{asOf: test.asOf, allocations: []Allocation{allocation}}
			got, err := NewProvider(&fakeEventStore{events: []SandboxEvent{event}}, client, func() time.Time { return cutoff }).PoolDetail(context.Background(), PoolQuery{Query: Query{Subject: "alice", Timeframe: test.timeframe}, PoolID: "ns-a:pool-a", Interval: test.interval})
			if err != nil || !got.Partial || !got.DataAsOf.Equal(test.asOf) || len(got.Buckets) != test.count || !got.Buckets[0].Start.Equal(cutoff.Add(-durationForTimeframe(test.timeframe))) || !got.Buckets[len(got.Buckets)-1].End.Equal(cutoff) {
				t.Fatalf("detail=%#v err=%v", got, err)
			}
			if !client.start.Equal(cutoff.Add(-durationForTimeframe(test.timeframe))) || !client.end.Equal(cutoff) {
				t.Fatalf("allocation query=[%s,%s)", client.start, client.end)
			}
		})
	}
}

func TestParsePoolIDMatchesDNSLabelPoolContract(t *testing.T) {
	t.Parallel()
	for _, id := range []string{"ns-a:pool-a", "ns-a:pool:extra", "ns-a:Pool", "ns-a:pool_", "ns-a:", "ns-a:" + strings.Repeat("a", 64)} {
		_, _, ok := parsePoolID(id)
		want := id == "ns-a:pool-a"
		if ok != want {
			t.Fatalf("parsePoolID(%q) ok=%t, want %t", id, ok, want)
		}
	}
}

func durationForTimeframe(timeframe Timeframe) time.Duration {
	switch timeframe {
	case Timeframe24H:
		return 24 * time.Hour
	case Timeframe7D:
		return 7 * 24 * time.Hour
	case Timeframe30D:
		return 30 * 24 * time.Hour
	default:
		panic("unsupported timeframe")
	}
}

func TestProviderRestoresAttributionAfterUnambiguousAdded(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 19, 10, 0, 0, 0, time.UTC)
	start := cutoff.Add(-24 * time.Hour)
	base := func(eventID, eventType string, observedAt time.Time) SandboxEvent {
		return SandboxEvent{EventID: mustUUID(eventID), Namespace: "ns-a", SandboxName: "sandbox", SandboxUID: "uid", PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm", EventType: eventType, ObservedAt: observedAt}
	}
	events := []SandboxEvent{
		base("00000000-0000-0000-0000-000000000095", "Added", start.Add(-time.Hour)),
		base("00000000-0000-0000-0000-000000000096", "Unexpected", start.Add(time.Hour)),
		base("00000000-0000-0000-0000-000000000097", "Added", start.Add(2*time.Hour)),
	}
	before := Allocation{Start: start, End: start.Add(time.Hour), Namespace: "ns-a", Pod: "virt-launcher-vm-a", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 1, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: gibibyte}
	after := Allocation{Start: start.Add(3 * time.Hour), End: start.Add(4 * time.Hour), Namespace: "ns-a", Pod: "virt-launcher-vm-a", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 1, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: gibibyte}
	got, err := NewProvider(&fakeEventStore{events: events}, &fakeAllocationClient{asOf: cutoff, allocations: []Allocation{before, after}}, func() time.Time { return cutoff }).Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H})
	if err != nil || !got.Partial || len(got.Pools) != 1 || got.Pools[0].CPU.Consumed != 2 {
		t.Fatalf("overview=%#v err=%v", got, err)
	}
}

type fakeReservationStore struct {
	facts   []ReservationFact
	asOf    time.Time
	partial bool
	err     error
	tenant  string
	cluster string
}

func (s *fakeReservationStore) Reservations(_ context.Context, tenant, cluster string, _, _ time.Time) ([]ReservationFact, time.Time, bool, error) {
	s.tenant, s.cluster = tenant, cluster
	return s.facts, s.asOf, s.partial, s.err
}

func TestProviderUsesVirtualReservationsForProvisionedTotals(t *testing.T) {
	t.Parallel()
	cutoff := time.Date(2026, 8, 22, 8, 0, 0, 0, time.UTC)
	events := &fakeEventStore{events: []SandboxEvent{{
		EventID: mustUUID("00000000-0000-0000-0000-000000000090"), Namespace: "ns-a", SandboxName: "sandbox-a", SandboxUID: "uid-a",
		PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm-a", EventType: "Added", ObservedAt: cutoff.Add(-25 * time.Hour),
	}}}
	allocations := &fakeAllocationClient{asOf: cutoff, allocations: []Allocation{{
		Start: cutoff.Add(-time.Hour), End: cutoff, Namespace: "ns-a", Pod: "virt-launcher-vm-a-abc",
		Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 2,
		RAMUsageAverageBytes: 3 * gibibyte, RAMRequestAverageBytes: 4 * gibibyte, CostUSD: 0.5,
	}}}
	reservations := &fakeReservationStore{asOf: cutoff, facts: []ReservationFact{{
		Namespace: "ns-a", SandboxUID: "uid-a", SandboxName: "sandbox-a", PoolName: "pool-a", Runtime: "kubevirt",
		HourStart: cutoff.Add(-time.Hour), HourEnd: cutoff,
		VirtualCPUCoreSeconds: 4 * 3600, VirtualMemoryByteSeconds: 8 * gibibyte * 3600,
	}}}
	provider := NewProviderWithReservations(events, allocations, reservations, "kopf-k3s", func() time.Time { return cutoff })

	got, err := provider.Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H})
	if err != nil {
		t.Fatal(err)
	}
	if reservations.tenant != "user-alice" || reservations.cluster != "kopf-k3s" || got.Partial || len(got.Pools) != 1 {
		t.Fatalf("overview=%#v reservations=%+v", got, reservations)
	}
	pool := got.Pools[0]
	if pool.CPU.Consumed != 1 || pool.CPU.Provisioned != 4 || pool.Memory.Consumed != 3 || pool.Memory.Provisioned != 8 || pool.CostUSD != 0.5 {
		t.Fatalf("pool=%#v", pool)
	}
}
