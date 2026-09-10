package usage

import (
	"context"
	"errors"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func cachedProviderFixture(now *time.Time) (*provider, *fakeEventStore, *fakeAllocationClient) {
	events := &fakeEventStore{events: []SandboxEvent{{
		EventID: mustUUID("00000000-0000-0000-0000-000000000091"), Namespace: "ns-a",
		SandboxName: "sandbox-a", SandboxUID: "uid-a", PoolName: "pool-a",
		Runtime: "kubevirt", VMName: "vm-a", EventType: "Added",
		ObservedAt: now.Add(-25 * time.Hour),
	}}}
	allocations := &fakeAllocationClient{asOf: now.UTC().Truncate(time.Hour), allocations: []Allocation{{
		Start: now.UTC().Truncate(time.Hour).Add(-time.Hour), End: now.UTC().Truncate(time.Hour),
		Namespace: "ns-a", Pod: "virt-launcher-vm-a-abc", Minutes: 60,
		CPUUsageAverage: 1, CPURequestAverage: 2,
		RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: 2 * gibibyte,
	}}}
	return NewProvider(events, allocations, func() time.Time { return *now }), events, allocations
}

func TestProviderCachesOverviewForPoolDetail(t *testing.T) {
	now := time.Date(2026, 8, 22, 4, 30, 0, 0, time.UTC)
	provider, events, allocations := cachedProviderFixture(&now)
	query := Query{ActorSubject: "alice", Subject: "alice", Timeframe: Timeframe24H}

	overview, err := provider.Overview(context.Background(), query)
	if err != nil || len(overview.Pools) != 1 {
		t.Fatalf("Overview() = %#v, %v", overview, err)
	}
	if _, err := provider.PoolDetail(context.Background(), PoolQuery{Query: query, PoolID: overview.Pools[0].ID, Interval: IntervalHour}); err != nil {
		t.Fatal(err)
	}
	if events.calls != 1 || allocations.calls != 1 {
		t.Fatalf("source calls = events %d allocations %d, want 1 each", events.calls, allocations.calls)
	}
}

func TestProviderCacheSeparatesSubjectsAndHourCutoffs(t *testing.T) {
	now := time.Date(2026, 8, 22, 4, 30, 0, 0, time.UTC)
	provider, _, allocations := cachedProviderFixture(&now)

	for _, subject := range []string{"alice", "bob"} {
		if _, err := provider.Overview(context.Background(), Query{Subject: subject, Timeframe: Timeframe24H}); err != nil {
			t.Fatal(err)
		}
	}
	now = now.Add(time.Hour)
	if _, err := provider.Overview(context.Background(), Query{Subject: "alice", Timeframe: Timeframe24H}); err != nil {
		t.Fatal(err)
	}
	if allocations.calls != 3 {
		t.Fatalf("allocation calls = %d, want 3", allocations.calls)
	}
}

func TestProviderCacheExpiresAndDoesNotCacheErrors(t *testing.T) {
	now := time.Date(2026, 8, 22, 4, 30, 0, 0, time.UTC)
	provider, _, allocations := cachedProviderFixture(&now)
	provider.cacheTTL = time.Minute
	query := Query{Subject: "alice", Timeframe: Timeframe24H}

	allocations.err = errors.New("temporary")
	if _, err := provider.Overview(context.Background(), query); err == nil {
		t.Fatal("Overview() error = nil")
	}
	allocations.err = nil
	if _, err := provider.Overview(context.Background(), query); err != nil {
		t.Fatal(err)
	}
	now = now.Add(2 * time.Minute)
	if _, err := provider.Overview(context.Background(), query); err != nil {
		t.Fatal(err)
	}
	if allocations.calls != 3 {
		t.Fatalf("allocation calls = %d, want 3", allocations.calls)
	}
}

type singleflightAllocationClient struct {
	calls atomic.Int64
}

func (client *singleflightAllocationClient) Allocations(_ context.Context, start, end time.Time, _ time.Duration, _ []string) ([]Allocation, time.Time, bool, error) {
	client.calls.Add(1)
	time.Sleep(25 * time.Millisecond)
	return []Allocation{{
		Start: end.Add(-time.Hour), End: end, Namespace: "ns-a", Pod: "virt-launcher-vm-a-abc",
		Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 2,
		RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: 2 * gibibyte,
	}}, end, false, nil
}

func TestProviderSingleflightsConcurrentLoads(t *testing.T) {
	now := time.Date(2026, 8, 22, 4, 30, 0, 0, time.UTC)
	events := benchmarkEventStore{}
	allocations := &singleflightAllocationClient{}
	provider := NewProvider(events, allocations, func() time.Time { return now })
	query := Query{Subject: "alice", Timeframe: Timeframe24H}

	var group sync.WaitGroup
	errorsSeen := make(chan error, 8)
	for index := 0; index < 8; index++ {
		group.Add(1)
		go func() {
			defer group.Done()
			_, err := provider.Overview(context.Background(), query)
			errorsSeen <- err
		}()
	}
	group.Wait()
	close(errorsSeen)
	for err := range errorsSeen {
		if err != nil {
			t.Fatal(err)
		}
	}
	if calls := allocations.calls.Load(); calls != 1 {
		t.Fatalf("allocation calls = %d, want 1", calls)
	}
}
