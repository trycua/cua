package metering

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"math"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/google/uuid"
)

const (
	CPUQuery    = "cua_osgym_sandbox_virtual_cpu_cores"
	MemoryQuery = "cua_osgym_sandbox_virtual_memory_bytes"
	ReadyQuery  = "cua_osgym_sandbox_ready"
	SourceQuery = `up{job="kube-state-metrics-required"}`
)

type RangeQuerier interface {
	QueryRange(context.Context, string, time.Time, time.Time, time.Duration) ([]Series, error)
}

type TenantResolver interface {
	ResolveTenant(context.Context, string, string, string) (string, bool, error)
}

type FactWriter interface {
	AppendFact(context.Context, HourFact) (bool, error)
	CompleteHour(context.Context, HourCompletion) (bool, error)
}

type HourFact struct {
	FactID                   uuid.UUID
	LogicalKey               string
	ClusterID                string
	CapsuleTenant            string
	Namespace                string
	SandboxUID               string
	SandboxName              string
	PoolName                 string
	Runtime                  string
	HourStart                time.Time
	HourEnd                  time.Time
	VirtualCPUCoreSeconds    float64
	VirtualMemoryByteSeconds float64
	ReadySeconds             float64
	CoveredSeconds           float64
	ScrapeIntervalSeconds    int
	SourceSHA256             string
	CollectionRunID          uuid.UUID
}

type HourCompletion struct {
	CollectionRunID     uuid.UUID
	LogicalKey          string
	ClusterID           string
	HourStart           time.Time
	HourEnd             time.Time
	CoveredSeconds      float64
	DiscoveredSandboxes int
	InsertedFacts       int
	UnchangedFacts      int
	SourceSHA256        string
}

type Collector struct {
	Prometheus        RangeQuerier
	Tenants           TenantResolver
	Writer            FactWriter
	ClusterID         string
	Step              time.Duration
	MaxSampleValidity time.Duration
	MinimumCoverage   time.Duration
}

type CollectResult struct {
	Discovered   int
	Inserted     int
	Unchanged    int
	Unattributed int
	Coverage     time.Duration
}

type sandboxIdentity struct {
	Namespace   string
	SandboxUID  string
	SandboxName string
	PoolName    string
	Runtime     string
}

type sandboxSamples struct {
	identity sandboxIdentity
	cpu      []Sample
	memory   []Sample
	ready    []Sample
}

func (c Collector) CollectHour(ctx context.Context, hourStart time.Time) (CollectResult, error) {
	if c.Prometheus == nil || c.Tenants == nil || c.Writer == nil || strings.TrimSpace(c.ClusterID) == "" {
		return CollectResult{}, fmt.Errorf("meter collector is not configured")
	}
	if c.Step <= 0 || c.MaxSampleValidity <= 0 || c.MinimumCoverage <= 0 {
		return CollectResult{}, fmt.Errorf("meter collector timing is invalid")
	}
	hourStart = hourStart.UTC()
	if !hourStart.Equal(hourStart.Truncate(time.Hour)) {
		return CollectResult{}, fmt.Errorf("meter hour must start on a UTC hour")
	}
	hourEnd := hourStart.Add(time.Hour)
	queryStart := hourStart.Add(-c.MaxSampleValidity)

	sourceSeries, err := c.Prometheus.QueryRange(ctx, SourceQuery, queryStart, hourEnd, c.Step)
	if err != nil {
		return CollectResult{}, fmt.Errorf("query KSM source coverage: %w", err)
	}
	if len(sourceSeries) != 1 {
		return CollectResult{}, fmt.Errorf("KSM source coverage returned %d series, want 1", len(sourceSeries))
	}
	sourceIntegral, err := Integrate(sourceSeries[0].Samples, hourStart, hourEnd, c.MaxSampleValidity)
	if err != nil {
		return CollectResult{}, fmt.Errorf("integrate KSM source coverage: %w", err)
	}
	coverage := time.Duration(sourceIntegral.ValueSeconds * float64(time.Second))
	if coverage < c.MinimumCoverage {
		return CollectResult{}, fmt.Errorf("KSM source coverage %s is below required %s", coverage, c.MinimumCoverage)
	}

	metricSeries := make(map[string][]Series, 3)
	for _, query := range []string{CPUQuery, MemoryQuery, ReadyQuery} {
		series, queryErr := c.Prometheus.QueryRange(ctx, query, queryStart, hourEnd, c.Step)
		if queryErr != nil {
			return CollectResult{}, fmt.Errorf("query %s: %w", query, queryErr)
		}
		metricSeries[query] = series
	}

	sandboxes, err := mergeSandboxSeries(metricSeries)
	if err != nil {
		return CollectResult{}, err
	}
	result := CollectResult{Discovered: len(sandboxes), Coverage: coverage}
	collectionRunID := uuid.New()
	factHashes := make([]string, 0, len(sandboxes))
	keys := make([]string, 0, len(sandboxes))
	for key := range sandboxes {
		keys = append(keys, key)
	}
	sort.Strings(keys)

	for _, key := range keys {
		sandbox := sandboxes[key]
		cpu, err := Integrate(sandbox.cpu, hourStart, hourEnd, c.MaxSampleValidity)
		if err != nil {
			return CollectResult{}, fmt.Errorf("integrate CPU for sandbox %s: %w", sandbox.identity.SandboxUID, err)
		}
		memory, err := Integrate(sandbox.memory, hourStart, hourEnd, c.MaxSampleValidity)
		if err != nil {
			return CollectResult{}, fmt.Errorf("integrate memory for sandbox %s: %w", sandbox.identity.SandboxUID, err)
		}
		if cpu.ValueSeconds < 0 || memory.ValueSeconds < 0 {
			return CollectResult{}, fmt.Errorf("sandbox %s has negative reservation samples", sandbox.identity.SandboxUID)
		}
		if math.Abs(cpu.PresentSeconds-memory.PresentSeconds) > c.Step.Seconds() {
			return CollectResult{}, fmt.Errorf("sandbox %s CPU and memory sample coverage diverged", sandbox.identity.SandboxUID)
		}
		ready, err := Integrate(sandbox.ready, hourStart, hourEnd, c.MaxSampleValidity)
		if err != nil {
			return CollectResult{}, fmt.Errorf("integrate readiness for sandbox %s: %w", sandbox.identity.SandboxUID, err)
		}
		tenant, attributed, err := c.Tenants.ResolveTenant(ctx, c.ClusterID, sandbox.identity.Namespace, sandbox.identity.SandboxUID)
		if err != nil {
			return CollectResult{}, fmt.Errorf("resolve tenant for sandbox %s: %w", sandbox.identity.SandboxUID, err)
		}
		if !attributed {
			result.Unattributed++
			continue
		}
		fact := HourFact{
			FactID:                   uuid.New(),
			LogicalKey:               c.ClusterID + "/" + sandbox.identity.SandboxUID + "/" + hourStart.Format(time.RFC3339),
			ClusterID:                c.ClusterID,
			CapsuleTenant:            tenant,
			Namespace:                sandbox.identity.Namespace,
			SandboxUID:               sandbox.identity.SandboxUID,
			SandboxName:              sandbox.identity.SandboxName,
			PoolName:                 sandbox.identity.PoolName,
			Runtime:                  sandbox.identity.Runtime,
			HourStart:                hourStart,
			HourEnd:                  hourEnd,
			VirtualCPUCoreSeconds:    cpu.ValueSeconds,
			VirtualMemoryByteSeconds: memory.ValueSeconds,
			ReadySeconds:             ready.ValueSeconds,
			CoveredSeconds:           sourceIntegral.ValueSeconds,
			ScrapeIntervalSeconds:    int(c.Step.Seconds()),
			CollectionRunID:          collectionRunID,
		}
		fact.SourceSHA256 = hashFactSource(fact, sandbox, sourceSeries[0].Samples)
		factHashes = append(factHashes, fact.SourceSHA256)
		inserted, err := c.Writer.AppendFact(ctx, fact)
		if err != nil {
			return CollectResult{}, fmt.Errorf("append sandbox %s reservation fact: %w", sandbox.identity.SandboxUID, err)
		}
		if inserted {
			result.Inserted++
		} else {
			result.Unchanged++
		}
	}
	sort.Strings(factHashes)
	completion := HourCompletion{
		CollectionRunID:     collectionRunID,
		LogicalKey:          c.ClusterID + "/" + hourStart.Format(time.RFC3339),
		ClusterID:           c.ClusterID,
		HourStart:           hourStart,
		HourEnd:             hourEnd,
		CoveredSeconds:      sourceIntegral.ValueSeconds,
		DiscoveredSandboxes: result.Discovered,
		InsertedFacts:       result.Inserted,
		UnchangedFacts:      result.Unchanged,
		SourceSHA256:        hashCollectionSource(c.ClusterID, hourStart, sourceSeries[0].Samples, factHashes),
	}
	if _, err := c.Writer.CompleteHour(ctx, completion); err != nil {
		return CollectResult{}, fmt.Errorf("complete reservation hour: %w", err)
	}
	return result, nil
}

func mergeSandboxSeries(metrics map[string][]Series) (map[string]*sandboxSamples, error) {
	out := make(map[string]*sandboxSamples)
	for _, metric := range []string{CPUQuery, MemoryQuery, ReadyQuery} {
		for _, series := range metrics[metric] {
			identity, err := identityFromLabels(series.Labels)
			if err != nil {
				return nil, fmt.Errorf("invalid %s labels: %w", metric, err)
			}
			key := identity.Namespace + "/" + identity.SandboxUID
			sandbox := out[key]
			if sandbox == nil {
				sandbox = &sandboxSamples{identity: identity}
				out[key] = sandbox
			} else if sandbox.identity != identity {
				return nil, fmt.Errorf("sandbox %s metric labels disagree", identity.SandboxUID)
			}
			var merged []Sample
			switch metric {
			case CPUQuery:
				merged, err = mergeMetricSamples(sandbox.cpu, series.Samples)
				sandbox.cpu = merged
			case MemoryQuery:
				merged, err = mergeMetricSamples(sandbox.memory, series.Samples)
				sandbox.memory = merged
			case ReadyQuery:
				merged, err = mergeMetricSamples(sandbox.ready, series.Samples)
				sandbox.ready = merged
			}
			if err != nil {
				return nil, fmt.Errorf("sandbox %s has conflicting %s series: %w", identity.SandboxUID, metric, err)
			}
		}
	}
	for _, sandbox := range out {
		if sandbox.cpu == nil || sandbox.memory == nil {
			return nil, fmt.Errorf("sandbox %s is missing CPU or memory series", sandbox.identity.SandboxUID)
		}
	}
	return out, nil
}

func mergeMetricSamples(existing, incoming []Sample) ([]Sample, error) {
	merged := append(append([]Sample(nil), existing...), incoming...)
	sort.Slice(merged, func(i, j int) bool { return merged[i].Timestamp.Before(merged[j].Timestamp) })
	compacted := merged[:0]
	for _, sample := range merged {
		if len(compacted) == 0 || !compacted[len(compacted)-1].Timestamp.Equal(sample.Timestamp) {
			compacted = append(compacted, sample)
			continue
		}
		if compacted[len(compacted)-1].Value != sample.Value {
			return nil, fmt.Errorf("timestamp %s has values %g and %g", sample.Timestamp.Format(time.RFC3339Nano), compacted[len(compacted)-1].Value, sample.Value)
		}
	}
	return compacted, nil
}

func identityFromLabels(labels map[string]string) (sandboxIdentity, error) {
	poolName := strings.TrimSpace(labels["pool"])
	if poolName == "" {
		poolName = strings.TrimSpace(labels["warmpool"])
	}
	identity := sandboxIdentity{
		Namespace:   strings.TrimSpace(labels["namespace"]),
		SandboxUID:  strings.TrimSpace(labels["sandbox_uid"]),
		SandboxName: strings.TrimSpace(labels["sandbox"]),
		PoolName:    poolName,
		Runtime:     strings.TrimSpace(labels["runtime"]),
	}
	if identity.Namespace == "" || identity.SandboxUID == "" || identity.SandboxName == "" || identity.PoolName == "" || identity.Runtime == "" {
		return sandboxIdentity{}, fmt.Errorf("required identity label is empty")
	}
	return identity, nil
}

func hashFactSource(fact HourFact, sandbox *sandboxSamples, source []Sample) string {
	hash := sha256.New()
	for _, value := range []string{
		fact.LogicalKey,
		fact.Namespace,
		fact.SandboxName,
		fact.PoolName,
		fact.Runtime,
		strconv.Itoa(fact.ScrapeIntervalSeconds),
	} {
		_, _ = hash.Write([]byte(value))
		_, _ = hash.Write([]byte{0})
	}
	for _, samples := range [][]Sample{sandbox.cpu, sandbox.memory, sandbox.ready, source} {
		ordered := append([]Sample(nil), samples...)
		sort.Slice(ordered, func(i, j int) bool { return ordered[i].Timestamp.Before(ordered[j].Timestamp) })
		for _, sample := range ordered {
			_, _ = hash.Write([]byte(strconv.FormatInt(sample.Timestamp.UnixNano(), 10)))
			_, _ = hash.Write([]byte{'='})
			_, _ = hash.Write([]byte(strconv.FormatFloat(sample.Value, 'g', -1, 64)))
			_, _ = hash.Write([]byte{0})
		}
		_, _ = hash.Write([]byte{0xff})
	}
	return hex.EncodeToString(hash.Sum(nil))
}

func hashCollectionSource(clusterID string, hourStart time.Time, source []Sample, factHashes []string) string {
	hash := sha256.New()
	_, _ = hash.Write([]byte(clusterID))
	_, _ = hash.Write([]byte{0})
	_, _ = hash.Write([]byte(hourStart.Format(time.RFC3339)))
	_, _ = hash.Write([]byte{0})
	for _, sample := range source {
		_, _ = hash.Write([]byte(strconv.FormatInt(sample.Timestamp.UnixNano(), 10)))
		_, _ = hash.Write([]byte{'='})
		_, _ = hash.Write([]byte(strconv.FormatFloat(sample.Value, 'g', -1, 64)))
		_, _ = hash.Write([]byte{0})
	}
	for _, factHash := range factHashes {
		_, _ = hash.Write([]byte(factHash))
		_, _ = hash.Write([]byte{0})
	}
	return hex.EncodeToString(hash.Sum(nil))
}
