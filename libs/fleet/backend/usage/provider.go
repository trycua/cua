package usage

import (
	"context"
	"errors"
	"fmt"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"

	"cyclops-cs-backend/identity"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/trace"
	"golang.org/x/sync/singleflight"
)

var (
	ErrPoolNotFound = errors.New("usage pool not found")
	dns1123Label    = regexp.MustCompile(`^[a-z0-9](?:[-a-z0-9]*[a-z0-9])?$`)
)

const (
	defaultUsageCacheTTL = 2 * time.Minute
	maxUsageCacheEntries = 256
)

type provider struct {
	events             EventStore
	allocations        AllocationClient
	reservations       ReservationStore
	reservationCluster string
	clock              func() time.Time
	cacheTTL           time.Duration
	cacheMu            sync.Mutex
	cache              map[usageCacheKey]usageCacheEntry
	loads              singleflight.Group
}

type usageCacheKey struct {
	subject   string
	timeframe Timeframe
	cutoff    time.Time
}

type usageCacheEntry struct {
	data      usageData
	expiresAt time.Time
}

type liveSegment struct {
	namespace string
	uid       string
	pool      string
	runtime   string
	vmName    string
	start     time.Time
	end       time.Time
}

func NewProvider(events EventStore, allocations AllocationClient, clock func() time.Time) *provider {
	return NewProviderWithReservations(events, allocations, nil, "", clock)
}

func NewProviderWithReservations(events EventStore, allocations AllocationClient, reservations ReservationStore, reservationCluster string, clock func() time.Time) *provider {
	if clock == nil {
		clock = time.Now
	}
	return &provider{
		events: events, allocations: allocations, reservations: reservations, reservationCluster: reservationCluster, clock: clock,
		cacheTTL: defaultUsageCacheTTL, cache: make(map[usageCacheKey]usageCacheEntry),
	}
}

func (p *provider) Overview(ctx context.Context, query Query) (OverviewResponse, error) {
	data, err := p.load(ctx, query)
	if err != nil {
		return OverviewResponse{}, err
	}
	return buildOverview(data), nil
}

func buildOverview(data usageData) OverviewResponse {
	response := OverviewResponse{DataAsOf: data.asOf, Partial: data.partial, Pools: make([]PoolSummary, 0)}
	for _, pool := range data.pools {
		response.Pools = append(response.Pools, PoolSummary{ID: pool.id, Name: pool.name, CPU: MetricTotals{Consumed: pool.cpuConsumed, Provisioned: pool.cpuProvisioned}, Memory: MetricTotals{Consumed: pool.memoryConsumed, Provisioned: pool.memoryProvisioned}, CostUSD: pool.costUSD})
	}
	return response
}

func (p *provider) PoolDetail(ctx context.Context, query PoolQuery) (PoolDetailResponse, error) {
	data, err := p.load(ctx, query.Query)
	if err != nil {
		return PoolDetailResponse{}, err
	}
	return buildPoolDetail(ctx, data, query.PoolID, query.Interval)
}

func buildPoolDetail(ctx context.Context, data usageData, poolID string, interval Interval) (PoolDetailResponse, error) {
	namespace, poolName, ok := parsePoolID(poolID)
	if !ok {
		return PoolDetailResponse{}, fmt.Errorf("invalid pool ID")
	}
	step, err := intervalDuration(interval)
	if err != nil {
		return PoolDetailResponse{}, err
	}
	pool, ok := findPool(data.pools, poolID)
	if !ok {
		return PoolDetailResponse{}, fmt.Errorf("%w: %s", ErrPoolNotFound, poolID)
	}
	_, bucketSpan := usageTracer().Start(ctx, "usage.buckets.build", trace.WithAttributes(
		attribute.String("usage.interval", string(interval)),
	))
	response := PoolDetailResponse{DataAsOf: data.asOf, Partial: data.partial, Pool: PoolIdentity{ID: pool.id, Name: pool.name}, Buckets: make([]Bucket, 0)}
	for start := data.start; start.Before(data.end); start = start.Add(step) {
		key := bucketKey{namespace: namespace, pool: poolName, start: start}
		totals := data.buckets[key]
		response.Buckets = append(response.Buckets, Bucket{Start: start, End: start.Add(step), CPUConsumed: totals.cpuConsumed, CPUProvisioned: totals.cpuProvisioned, MemoryConsumed: totals.memoryConsumed, MemoryProvisioned: totals.memoryProvisioned})
	}
	bucketSpan.SetAttributes(attribute.Int("usage.bucket_count", len(response.Buckets)))
	bucketSpan.End()
	return response, nil
}

type usageData struct {
	start   time.Time
	end     time.Time
	asOf    time.Time
	partial bool
	pools   []usageTotals
	buckets map[bucketKey]usageTotals
}

type usageTotals struct {
	id                string
	name              string
	cpuConsumed       float64
	cpuProvisioned    float64
	memoryConsumed    float64
	memoryProvisioned float64
	costUSD           float64
}

type bucketKey struct {
	namespace string
	pool      string
	start     time.Time
}

func (p *provider) load(ctx context.Context, query Query) (usageData, error) {
	now := p.clock().UTC()
	key := usageCacheKey{subject: query.Subject, timeframe: query.Timeframe, cutoff: now.Truncate(time.Hour)}
	ctx, span := usageTracer().Start(ctx, "usage.load", trace.WithAttributes(
		attribute.String("usage.timeframe", string(query.Timeframe)),
		attribute.Bool("usage.admin", query.Admin),
		attribute.Bool("usage.subject_override", query.ActorSubject != "" && query.ActorSubject != query.Subject),
	))
	defer span.End()

	if cached, ok := p.cached(key, now); ok {
		span.SetAttributes(attribute.Bool("usage.cache.hit", true))
		recordUsageCache(ctx, query.Timeframe, "hit")
		return cached, nil
	}
	span.SetAttributes(attribute.Bool("usage.cache.hit", false))

	loaded, err, shared := p.loads.Do(cacheFlightKey(key), func() (any, error) {
		if cached, ok := p.cached(key, p.clock().UTC()); ok {
			return cached, nil
		}
		data, loadErr := p.loadUncached(ctx, query, now)
		if loadErr == nil {
			p.storeCached(key, data, p.clock().UTC())
		}
		return data, loadErr
	})
	cacheOutcome := "miss"
	if shared {
		cacheOutcome = "shared"
	}
	recordUsageCache(ctx, query.Timeframe, cacheOutcome)
	if err != nil {
		return usageData{}, err
	}
	return loaded.(usageData), nil
}

func (p *provider) loadUncached(ctx context.Context, query Query, now time.Time) (data usageData, err error) {
	ctx, span := usageTracer().Start(ctx, "usage.load.uncached")
	defer span.End()
	started := time.Now()
	defer func() { recordUsageLoad(ctx, query.Timeframe, time.Since(started), err) }()

	if p.events == nil || p.allocations == nil {
		err = fmt.Errorf("usage provider is not configured")
		markUsageSpanError(span, "usage provider unavailable")
		return usageData{}, err
	}
	start, end, step, err := usageWindow(query.Timeframe, now)
	if err != nil {
		markUsageSpanError(span, "invalid usage window")
		return usageData{}, err
	}
	span.SetAttributes(attribute.Int64("usage.window_seconds", int64(end.Sub(start).Seconds())))

	eventsCtx, eventsSpan := usageTracer().Start(ctx, "usage.events.query")
	events, err := p.events.Events(eventsCtx, identity.PersonalGroup(eventsCtx, query.Subject), start, end)
	if err != nil {
		markUsageSpanError(eventsSpan, "usage event query failed")
		eventsSpan.End()
		markUsageSpanError(span, "usage event query failed")
		return usageData{}, fmt.Errorf("read usage events: %w", err)
	}
	eventsSpan.SetAttributes(attribute.Int("usage.event_count", len(events)))
	eventsSpan.End()

	_, segmentsSpan := usageTracer().Start(ctx, "usage.segments.build", trace.WithAttributes(
		attribute.Int("usage.event_count", len(events)),
	))
	segments, namespaces, partial := buildSegments(events, start, end)
	segmentsSpan.SetAttributes(
		attribute.Int("usage.segment_count", len(segments)),
		attribute.Int("usage.namespace_count", len(namespaces)),
		attribute.Bool("usage.partial", partial),
	)
	segmentsSpan.End()

	data = usageData{start: start, end: end, asOf: end, partial: partial, pools: make([]usageTotals, 0), buckets: make(map[bucketKey]usageTotals)}
	if len(segments) == 0 || len(namespaces) == 0 {
		span.SetAttributes(
			attribute.Int("usage.pool_count", 0),
			attribute.Int("usage.bucket_count", 0),
			attribute.Bool("usage.partial", data.partial),
		)
		return data, nil
	}

	allocationsCtx, allocationsSpan := usageTracer().Start(ctx, "usage.allocations.query", trace.WithAttributes(
		attribute.Int("usage.namespace_count", len(namespaces)),
		attribute.Int64("usage.step_seconds", int64(step.Seconds())),
	))
	allocations, asOf, sourcePartial, err := p.allocations.Allocations(allocationsCtx, start, end, step, namespaces)
	if err != nil {
		markUsageSpanError(allocationsSpan, "allocation query failed")
		allocationsSpan.End()
		markUsageSpanError(span, "allocation query failed")
		return usageData{}, fmt.Errorf("read allocation data: %w", err)
	}
	allocationsSpan.SetAttributes(
		attribute.Int("usage.allocation_count", len(allocations)),
		attribute.Bool("usage.partial", sourcePartial),
	)
	allocationsSpan.End()

	if asOf.IsZero() {
		asOf = start
		sourcePartial = true
	}
	data.asOf = minTime(end, asOf.UTC())
	if data.asOf.Before(start) {
		data.asOf = start
		sourcePartial = true
	}
	data.partial = data.partial || sourcePartial || data.asOf.Before(end)

	_, attributionSpan := usageTracer().Start(ctx, "usage.allocations.attribute", trace.WithAttributes(
		attribute.Int("usage.allocation_count", len(allocations)),
	))
	poolTotals := make(map[string]usageTotals)
	for _, allocation := range allocations {
		matched, allocationPartial := attributeAllocation(allocation, segments, start, end, step, poolTotals, data.buckets)
		if !matched || allocationPartial {
			data.partial = true
		}
	}
	if p.reservations != nil {
		reservationCtx, reservationSpan := usageTracer().Start(ctx, "usage.reservations.query")
		facts, reservationAsOf, reservationPartial, reservationErr := p.reservations.Reservations(reservationCtx, identity.PersonalGroup(reservationCtx, query.Subject), p.reservationCluster, start, end)
		if reservationErr != nil {
			markUsageSpanError(reservationSpan, "reservation fact query failed")
			reservationSpan.End()
			markUsageSpanError(span, "reservation fact query failed")
			return usageData{}, fmt.Errorf("read reservation facts: %w", reservationErr)
		}
		reservationSpan.SetAttributes(attribute.Int("usage.reservation_fact_count", len(facts)))
		reservationSpan.End()
		if reservationAsOf.Before(data.asOf) {
			data.asOf = maxTime(start, reservationAsOf.UTC())
		}
		data.partial = data.partial || reservationPartial || data.asOf.Before(end)
		if err := applyReservationFacts(facts, start, end, step, poolTotals, data.buckets); err != nil {
			markUsageSpanError(span, "reservation fact attribution failed")
			return usageData{}, err
		}
	}
	for _, totals := range poolTotals {
		if !totals.isZero() {
			data.pools = append(data.pools, totals)
		}
	}
	disambiguatePoolNames(data.pools)
	sort.Slice(data.pools, func(i, j int) bool { return data.pools[i].id < data.pools[j].id })
	attributionSpan.SetAttributes(
		attribute.Int("usage.pool_count", len(data.pools)),
		attribute.Int("usage.bucket_count", len(data.buckets)),
		attribute.Bool("usage.partial", data.partial),
	)
	attributionSpan.End()
	span.SetAttributes(
		attribute.Int("usage.pool_count", len(data.pools)),
		attribute.Int("usage.bucket_count", len(data.buckets)),
		attribute.Bool("usage.partial", data.partial),
	)
	return data, nil
}

func cacheFlightKey(key usageCacheKey) string {
	return key.subject + "\x00" + string(key.timeframe) + "\x00" + key.cutoff.Format(time.RFC3339)
}

func (p *provider) cached(key usageCacheKey, now time.Time) (usageData, bool) {
	if p.cacheTTL <= 0 {
		return usageData{}, false
	}
	p.cacheMu.Lock()
	defer p.cacheMu.Unlock()
	entry, ok := p.cache[key]
	if !ok {
		return usageData{}, false
	}
	if !now.Before(entry.expiresAt) {
		delete(p.cache, key)
		return usageData{}, false
	}
	return entry.data, true
}

func (p *provider) storeCached(key usageCacheKey, data usageData, now time.Time) {
	if p.cacheTTL <= 0 {
		return
	}
	p.cacheMu.Lock()
	defer p.cacheMu.Unlock()
	if len(p.cache) >= maxUsageCacheEntries {
		var oldestKey usageCacheKey
		var oldestExpiry time.Time
		for candidate, entry := range p.cache {
			if !now.Before(entry.expiresAt) {
				delete(p.cache, candidate)
				continue
			}
			if oldestExpiry.IsZero() || entry.expiresAt.Before(oldestExpiry) {
				oldestKey, oldestExpiry = candidate, entry.expiresAt
			}
		}
		if len(p.cache) >= maxUsageCacheEntries && !oldestExpiry.IsZero() {
			delete(p.cache, oldestKey)
		}
	}
	p.cache[key] = usageCacheEntry{data: data, expiresAt: now.Add(p.cacheTTL)}
}

func usageWindow(timeframe Timeframe, now time.Time) (time.Time, time.Time, time.Duration, error) {
	cutoff := now.UTC().Truncate(time.Hour)
	switch timeframe {
	case Timeframe24H:
		return cutoff.Add(-24 * time.Hour), cutoff, time.Hour, nil
	case Timeframe7D:
		return cutoff.Add(-7 * 24 * time.Hour), cutoff, time.Hour, nil
	case Timeframe30D:
		return cutoff.Add(-30 * 24 * time.Hour), cutoff, 24 * time.Hour, nil
	default:
		return time.Time{}, time.Time{}, 0, fmt.Errorf("invalid usage timeframe")
	}
}

func intervalDuration(interval Interval) (time.Duration, error) {
	switch interval {
	case IntervalHour:
		return time.Hour, nil
	case IntervalDay:
		return 24 * time.Hour, nil
	default:
		return 0, fmt.Errorf("invalid usage interval")
	}
}

func parsePoolID(id string) (string, string, bool) {
	parts := strings.Split(id, ":")
	if len(parts) != 2 || !dns1123Label.MatchString(parts[0]) || !validPoolName(parts[1]) {
		return "", "", false
	}
	return parts[0], parts[1], true
}

func validPoolName(name string) bool {
	return len(name) <= 63 && dns1123Label.MatchString(name)
}

func buildSegments(events []SandboxEvent, start, end time.Time) ([]liveSegment, []string, bool) {
	bySandbox := make(map[string][]SandboxEvent)
	seen := make(map[[16]byte]struct{}, len(events))
	partial := false
	for _, event := range events {
		if _, ok := seen[event.EventID]; ok {
			continue
		}
		seen[event.EventID] = struct{}{}
		if err := validateSandboxEvent(event); err != nil || !event.ObservedAt.Before(end) {
			partial = true
			continue
		}
		key := event.Namespace + "\x00" + event.SandboxUID
		bySandbox[key] = append(bySandbox[key], event)
	}
	segments := make([]liveSegment, 0)
	namespaceSet := make(map[string]struct{})
	for _, timeline := range bySandbox {
		sort.SliceStable(timeline, func(i, j int) bool {
			return timeline[i].ObservedAt.Before(timeline[j].ObservedAt)
		})
		var active *SandboxEvent
		activeStart := start
		certain := false
		appendActive := func(until time.Time) {
			if active == nil || !certain || !until.After(activeStart) {
				return
			}
			segmentEnd := minTime(until, end)
			if segmentEnd.After(activeStart) {
				segments = append(segments, liveSegment{namespace: active.Namespace, uid: active.SandboxUID, pool: active.PoolName, runtime: active.Runtime, vmName: active.VMName, start: activeStart, end: segmentEnd})
				namespaceSet[active.Namespace] = struct{}{}
			}
		}
		invalidate := func(at time.Time) {
			appendActive(at)
			active = nil
			activeStart = at
			certain = false
		}
		for index := 0; index < len(timeline); {
			event := timeline[index]
			at := event.ObservedAt.UTC()
			if index+1 < len(timeline) && at.Equal(timeline[index+1].ObservedAt) {
				partial = true
				invalidate(at)
				for index < len(timeline) && at.Equal(timeline[index].ObservedAt.UTC()) {
					index++
				}
				continue
			}
			isBaseline := index == 0 && at.Before(start)
			if isBaseline {
				switch strings.ToLower(event.EventType) {
				case "added", "modified":
					copy := event
					active = &copy
					activeStart = start
					certain = true
				case "deleted":
					active = nil
					activeStart = start
					certain = true
				default:
					partial = true
					invalidate(start)
				}
				index++
				continue
			}
			if !at.Before(start) && index == 0 {
				partial = true
			}
			switch strings.ToLower(event.EventType) {
			case "added":
				if active != nil && certain {
					partial = true
					invalidate(at)
					index++
					continue
				}
				copy := event
				active = &copy
				activeStart = maxTime(at, start)
				certain = true
			case "modified":
				if active == nil || !certain {
					partial = true
					invalidate(at)
					index++
					continue
				}
				appendActive(at)
				copy := event
				active = &copy
				activeStart = maxTime(at, start)
			case "deleted":
				if active == nil || !certain {
					partial = true
					invalidate(at)
					index++
					continue
				}
				appendActive(at)
				active = nil
				activeStart = at
				certain = true
			default:
				partial = true
				invalidate(at)
			}
			index++
		}
		appendActive(end)
	}
	namespaces := make([]string, 0, len(namespaceSet))
	for namespace := range namespaceSet {
		namespaces = append(namespaces, namespace)
	}
	sort.Strings(namespaces)
	return segments, namespaces, partial
}

func findPool(pools []usageTotals, id string) (usageTotals, bool) {
	for _, pool := range pools {
		if pool.id == id {
			return pool, true
		}
	}
	return usageTotals{}, false
}

func (totals usageTotals) isZero() bool {
	return totals.cpuConsumed == 0 && totals.cpuProvisioned == 0 && totals.memoryConsumed == 0 && totals.memoryProvisioned == 0 && totals.costUSD == 0
}

func disambiguatePoolNames(pools []usageTotals) {
	counts := make(map[string]int, len(pools))
	for _, pool := range pools {
		counts[pool.name]++
	}
	for index := range pools {
		if counts[pools[index].name] > 1 {
			namespace, _, _ := strings.Cut(pools[index].id, ":")
			pools[index].name = namespace + "/" + pools[index].name
		}
	}
}

func attributeAllocation(allocation Allocation, segments []liveSegment, windowStart, windowEnd time.Time, bucketDuration time.Duration, pools map[string]usageTotals, buckets map[bucketKey]usageTotals) (bool, bool) {
	if allocation.Start.IsZero() || allocation.End.IsZero() || !allocation.End.After(allocation.Start) || allocation.Start.Before(windowStart) || allocation.End.After(windowEnd) || allocation.Minutes < 0 {
		return false, true
	}
	matches := matchingSegments(allocation, segments)
	if len(matches) == 0 {
		return false, true
	}
	covered := 0.0
	for _, match := range matches {
		overlapStart := maxTime(allocation.Start, match.start)
		overlapEnd := minTime(allocation.End, match.end)
		if overlapEnd.After(overlapStart) {
			covered += overlapEnd.Sub(overlapStart).Seconds()
		}
	}
	partial := covered+0.000001 < allocation.End.Sub(allocation.Start).Seconds()
	for _, match := range matches {
		for bucketStart := bucketStartFor(allocation.Start, windowStart, bucketDuration); bucketStart.Before(allocation.End); bucketStart = bucketStart.Add(bucketDuration) {
			bucketEnd := bucketStart.Add(bucketDuration)
			overlapStart := maxTime(maxTime(allocation.Start, match.start), bucketStart)
			overlapEnd := minTime(minTime(allocation.End, match.end), bucketEnd)
			if !overlapEnd.After(overlapStart) {
				continue
			}
			fraction := overlapEnd.Sub(overlapStart).Seconds() / allocation.End.Sub(allocation.Start).Seconds()
			minutes := allocation.Minutes * fraction
			cpuConsumed := allocation.CPUUsageAverage * minutes / 60
			cpuProvisioned := allocation.CPURequestAverage * minutes / 60
			memoryConsumed := allocation.RAMUsageAverageBytes * minutes / 60 / gibibyte
			memoryProvisioned := allocation.RAMRequestAverageBytes * minutes / 60 / gibibyte
			costUSD := allocation.CostUSD * fraction
			id := match.namespace + ":" + match.pool
			pool := pools[id]
			pool.id, pool.name = id, match.pool
			pool.cpuConsumed += cpuConsumed
			pool.cpuProvisioned += cpuProvisioned
			pool.memoryConsumed += memoryConsumed
			pool.memoryProvisioned += memoryProvisioned
			pool.costUSD += costUSD
			pools[id] = pool
			key := bucketKey{namespace: match.namespace, pool: match.pool, start: bucketStart}
			bucket := buckets[key]
			bucket.cpuConsumed += cpuConsumed
			bucket.cpuProvisioned += cpuProvisioned
			bucket.memoryConsumed += memoryConsumed
			bucket.memoryProvisioned += memoryProvisioned
			buckets[key] = bucket
		}
	}
	return true, partial
}

func bucketStartFor(value, origin time.Time, duration time.Duration) time.Time {
	value, origin = value.UTC(), origin.UTC()
	if value.Before(origin) {
		return origin
	}
	return origin.Add((value.Sub(origin) / duration) * duration)
}

func matchingSegments(allocation Allocation, segments []liveSegment) []liveSegment {
	matches := make([]liveSegment, 0)
	for _, segment := range segments {
		if segment.namespace != allocation.Namespace || !allocation.End.After(segment.start) || !segment.end.After(allocation.Start) || !strings.EqualFold(segment.runtime, "kubevirt") {
			continue
		}
		matches = append(matches, segment)
	}
	if len(matches) == 0 {
		return nil
	}
	best := -1
	for _, segment := range matches {
		if kubeVirtPodMatches(allocation.Pod, segment.vmName) && len(segment.vmName) > best {
			best = len(segment.vmName)
		}
	}
	if best < 0 {
		return nil
	}
	result := matches[:0]
	for _, segment := range matches {
		if len(segment.vmName) == best && kubeVirtPodMatches(allocation.Pod, segment.vmName) {
			result = append(result, segment)
		}
	}
	// Adjacent events for the same sandbox can produce multiple temporal pieces;
	// reject competing sandboxes that overlap the same allocation instant.
	for index, left := range result {
		for _, right := range result[index+1:] {
			if left.uid != right.uid && left.end.After(right.start) && right.end.After(left.start) {
				return nil
			}
		}
	}
	return result
}

func kubeVirtPodMatches(pod, vmName string) bool {
	prefix := "virt-launcher-" + vmName + "-"
	return strings.HasPrefix(pod, prefix) && len(pod) > len(prefix)
}

func minTime(left, right time.Time) time.Time {
	if left.Before(right) {
		return left
	}
	return right
}
func maxTime(left, right time.Time) time.Time {
	if left.After(right) {
		return left
	}
	return right
}

var _ Provider = (*provider)(nil)

func applyReservationFacts(facts []ReservationFact, windowStart, windowEnd time.Time, bucketDuration time.Duration, pools map[string]usageTotals, buckets map[bucketKey]usageTotals) error {
	for id, pool := range pools {
		pool.cpuProvisioned = 0
		pool.memoryProvisioned = 0
		pools[id] = pool
	}
	for key, bucket := range buckets {
		bucket.cpuProvisioned = 0
		bucket.memoryProvisioned = 0
		buckets[key] = bucket
	}
	for _, fact := range facts {
		if fact.Namespace == "" || !validPoolName(fact.PoolName) || !fact.HourEnd.Equal(fact.HourStart.Add(time.Hour)) || fact.HourStart.Before(windowStart) || fact.HourEnd.After(windowEnd) || fact.VirtualCPUCoreSeconds < 0 || fact.VirtualMemoryByteSeconds < 0 {
			return fmt.Errorf("invalid reservation fact")
		}
		cpuCoreHours := fact.VirtualCPUCoreSeconds / 3600
		memoryGiBHours := fact.VirtualMemoryByteSeconds / 3600 / gibibyte
		id := fact.Namespace + ":" + fact.PoolName
		pool := pools[id]
		pool.id, pool.name = id, fact.PoolName
		pool.cpuProvisioned += cpuCoreHours
		pool.memoryProvisioned += memoryGiBHours
		pools[id] = pool
		key := bucketKey{namespace: fact.Namespace, pool: fact.PoolName, start: bucketStartFor(fact.HourStart, windowStart, bucketDuration)}
		bucket := buckets[key]
		bucket.cpuProvisioned += cpuCoreHours
		bucket.memoryProvisioned += memoryGiBHours
		buckets[key] = bucket
	}
	return nil
}
