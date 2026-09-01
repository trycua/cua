package metering

import (
	"context"
	"strings"
	"testing"
	"time"
)

type fakeQuerier map[string][]Series

func (f fakeQuerier) QueryRange(_ context.Context, query string, _, _ time.Time, _ time.Duration) ([]Series, error) {
	return f[query], nil
}

type fakeTenantResolver struct{ tenant string }

func (f fakeTenantResolver) ResolveTenant(context.Context, string, string, string) (string, bool, error) {
	return f.tenant, f.tenant != "", nil
}

type fakeFactWriter struct{ facts []HourFact }

func (f *fakeFactWriter) AppendFact(_ context.Context, fact HourFact) (bool, error) {
	f.facts = append(f.facts, fact)
	return true, nil
}

func (f *fakeFactWriter) CompleteHour(context.Context, HourCompletion) (bool, error) {
	return true, nil
}

func TestCollectorMaterializesReservationHour(t *testing.T) {
	start := time.Date(2026, 8, 22, 7, 0, 0, 0, time.UTC)
	labels := map[string]string{
		"namespace":   "tenant-a",
		"sandbox":     "sandbox-a",
		"sandbox_uid": "uid-a",
		"pool":        "pool-a",
		"runtime":     "kubevirt",
	}
	writer := &fakeFactWriter{}
	collector := Collector{
		Prometheus: fakeQuerier{
			SourceQuery: {{Samples: constantSamples(start.Add(-15*time.Second), start.Add(time.Hour), 15*time.Second, 1)}},
			CPUQuery:    {{Labels: labels, Samples: constantSamples(start.Add(-15*time.Second), start.Add(time.Hour), 15*time.Second, 4)}},
			MemoryQuery: {{Labels: labels, Samples: constantSamples(start.Add(-15*time.Second), start.Add(time.Hour), 15*time.Second, float64(8<<30))}},
			ReadyQuery:  {{Labels: labels, Samples: constantSamples(start.Add(-15*time.Second), start.Add(time.Hour), 15*time.Second, 1)}},
		},
		Tenants:           fakeTenantResolver{tenant: "personal:user-a"},
		Writer:            writer,
		ClusterID:         "kopf-k3s",
		Step:              15 * time.Second,
		MaxSampleValidity: time.Minute,
		MinimumCoverage:   3570 * time.Second,
	}

	result, err := collector.CollectHour(context.Background(), start)
	if err != nil {
		t.Fatal(err)
	}
	if result.Discovered != 1 || result.Inserted != 1 || len(writer.facts) != 1 {
		t.Fatalf("result = %+v, facts = %d", result, len(writer.facts))
	}
	fact := writer.facts[0]
	if fact.VirtualCPUCoreSeconds != 4*3600 || fact.VirtualMemoryByteSeconds != float64(8<<30)*3600 || fact.ReadySeconds != 3600 || fact.CoveredSeconds != 3600 {
		t.Fatalf("fact quantities = %+v", fact)
	}
	if fact.CapsuleTenant != "personal:user-a" || fact.SourceSHA256 == "" || !strings.Contains(fact.LogicalKey, "uid-a") {
		t.Fatalf("fact identity = %+v", fact)
	}
}

func TestCollectorSkipsUnattributedSandbox(t *testing.T) {
	start := time.Date(2026, 8, 22, 7, 0, 0, 0, time.UTC)
	labels := map[string]string{
		"namespace": "legacy-system", "sandbox": "sandbox-a", "sandbox_uid": "uid-a",
		"pool": "pool-a", "runtime": "macos",
	}
	writer := &fakeFactWriter{}
	collector := Collector{
		Prometheus: fakeQuerier{
			SourceQuery: {{Samples: constantSamples(start.Add(-15*time.Second), start.Add(time.Hour), 15*time.Second, 1)}},
			CPUQuery:    {{Labels: labels, Samples: constantSamples(start, start.Add(time.Hour), 15*time.Second, 4)}},
			MemoryQuery: {{Labels: labels, Samples: constantSamples(start, start.Add(time.Hour), 15*time.Second, float64(4<<30))}},
		},
		Tenants: fakeTenantResolver{}, Writer: writer, ClusterID: "kopf-k3s",
		Step: 15 * time.Second, MaxSampleValidity: time.Minute, MinimumCoverage: 3570 * time.Second,
	}

	result, err := collector.CollectHour(context.Background(), start)
	if err != nil {
		t.Fatal(err)
	}
	if result.Discovered != 1 || result.Unattributed != 1 || result.Inserted != 0 || len(writer.facts) != 0 {
		t.Fatalf("result = %+v, facts = %d", result, len(writer.facts))
	}
}

func TestCollectorFailsClosedOnSourceCoverage(t *testing.T) {
	start := time.Date(2026, 8, 22, 7, 0, 0, 0, time.UTC)
	collector := Collector{
		Prometheus:        fakeQuerier{SourceQuery: {{Samples: []Sample{{Timestamp: start, Value: 1}}}}},
		Tenants:           fakeTenantResolver{tenant: "personal:user-a"},
		Writer:            &fakeFactWriter{},
		ClusterID:         "kopf-k3s",
		Step:              15 * time.Second,
		MaxSampleValidity: time.Minute,
		MinimumCoverage:   3570 * time.Second,
	}
	_, err := collector.CollectHour(context.Background(), start)
	if err == nil || !strings.Contains(err.Error(), "below required") {
		t.Fatalf("error = %v, want coverage rejection", err)
	}
}

func TestMergeSandboxSeriesStitchesLabelChurn(t *testing.T) {
	start := time.Date(2026, 8, 22, 10, 0, 0, 0, time.UTC)
	labels := map[string]string{
		"namespace": "tenant-a", "sandbox": "sandbox-a", "sandbox_uid": "uid-a",
		"pool": "pool-a", "runtime": "kubevirt",
	}
	labelsWithWarmPool := map[string]string{
		"namespace": "tenant-a", "sandbox": "sandbox-a", "sandbox_uid": "uid-a",
		"pool": "pool-a", "warmpool": "pool-a", "runtime": "kubevirt",
	}
	metrics := map[string][]Series{
		CPUQuery: {
			{Labels: labelsWithWarmPool, Samples: []Sample{{Timestamp: start, Value: 4}}},
			{Labels: labels, Samples: []Sample{{Timestamp: start.Add(15 * time.Second), Value: 4}}},
		},
		MemoryQuery: {{Labels: labels, Samples: []Sample{{Timestamp: start, Value: float64(4 << 30)}}}},
	}

	sandboxes, err := mergeSandboxSeries(metrics)
	if err != nil {
		t.Fatal(err)
	}
	if got := sandboxes["tenant-a/uid-a"].cpu; len(got) != 2 || !got[0].Timestamp.Equal(start) || !got[1].Timestamp.Equal(start.Add(15*time.Second)) {
		t.Fatalf("merged CPU samples = %+v", got)
	}
}

func TestMergeMetricSamplesRejectsConflictingTimestamp(t *testing.T) {
	timestamp := time.Date(2026, 8, 22, 10, 0, 0, 0, time.UTC)
	_, err := mergeMetricSamples(
		[]Sample{{Timestamp: timestamp, Value: 4}},
		[]Sample{{Timestamp: timestamp, Value: 8}},
	)
	if err == nil || !strings.Contains(err.Error(), "has values 4 and 8") {
		t.Fatalf("error = %v, want conflicting values", err)
	}
}

func TestIdentityFromLabelsFallsBackToWarmPoolLabel(t *testing.T) {
	identity, err := identityFromLabels(map[string]string{
		"namespace":   "tenant-a",
		"sandbox":     "sandbox-a",
		"sandbox_uid": "uid-a",
		"warmpool":    "pool-a",
		"runtime":     "macos",
	})
	if err != nil {
		t.Fatal(err)
	}
	if identity.PoolName != "pool-a" {
		t.Fatalf("pool = %q, want pool-a", identity.PoolName)
	}
}

func constantSamples(start, end time.Time, step time.Duration, value float64) []Sample {
	var samples []Sample
	for timestamp := start; !timestamp.After(end); timestamp = timestamp.Add(step) {
		samples = append(samples, Sample{Timestamp: timestamp, Value: value})
	}
	return samples
}
