package usage

import (
	"context"
	"testing"
	"time"
)

type benchmarkEventStore struct{}

func (benchmarkEventStore) Events(_ context.Context, _ string, start, end time.Time) ([]SandboxEvent, error) {
	return []SandboxEvent{{EventID: mustUUID("00000000-0000-0000-0000-000000000099"), SandboxName: "sandbox-a", Namespace: "ns-a", SandboxUID: "uid-a", PoolName: "pool-a", Runtime: "kubevirt", VMName: "vm-a", EventType: "Added", ObservedAt: start.Add(-time.Hour)}}, nil
}

type benchmarkAllocationClient struct {
	delay time.Duration
	calls int
}

func (client *benchmarkAllocationClient) Allocations(_ context.Context, start, end time.Time, _ time.Duration, _ []string) ([]Allocation, time.Time, bool, error) {
	client.calls++
	time.Sleep(client.delay)
	return []Allocation{{Start: end.Add(-time.Hour), End: end, Namespace: "ns-a", Pod: "virt-launcher-vm-a-abc", Minutes: 60, CPUUsageAverage: 1, CPURequestAverage: 2, RAMUsageAverageBytes: gibibyte, RAMRequestAverageBytes: 2 * gibibyte}}, end, false, nil
}

func BenchmarkDashboardLoad(benchmark *testing.B) {
	now := time.Date(2026, 8, 22, 4, 30, 0, 0, time.UTC)
	query := Query{ActorSubject: "user", Subject: "user", Timeframe: Timeframe24H}

	benchmark.ReportAllocs()
	benchmark.ResetTimer()
	for index := 0; index < benchmark.N; index++ {
		allocations := &benchmarkAllocationClient{delay: 20 * time.Millisecond}
		provider := NewProvider(benchmarkEventStore{}, allocations, func() time.Time { return now })
		before := allocations.calls
		overview, err := provider.Overview(context.Background(), query)
		if err != nil || len(overview.Pools) == 0 {
			benchmark.Fatalf("Overview() = %v, %v", overview, err)
		}
		if _, err := provider.PoolDetail(context.Background(), PoolQuery{Query: query, PoolID: overview.Pools[0].ID, Interval: IntervalHour}); err != nil {
			benchmark.Fatal(err)
		}
		benchmark.ReportMetric(float64(allocations.calls-before), "allocation_queries/op")
	}
}
