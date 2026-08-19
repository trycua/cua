package usage

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"math"
	"net/http"
	"net/url"
	"regexp"
	"sort"
	"strings"
	"time"
)

const maxAllocationRows = 100000

var dns1123Label = regexp.MustCompile(`^[a-z0-9](?:[-a-z0-9]*[a-z0-9])?$`)

type openCostClient struct {
	baseURL          *url.URL
	httpClient       *http.Client
	timeout          time.Duration
	maxResponseBytes int64
}

func NewOpenCostClient(rawURL string, timeout time.Duration, maxResponseBytes int64) (*openCostClient, error) {
	baseURL, err := url.Parse(rawURL)
	if err != nil {
		return nil, newSanitizedError("invalid OpenCost URL", err)
	}
	if (baseURL.Scheme != "http" && baseURL.Scheme != "https") || baseURL.Host == "" || baseURL.RawQuery != "" || baseURL.Fragment != "" || baseURL.User != nil {
		return nil, fmt.Errorf("invalid OpenCost URL")
	}
	if timeout <= 0 || maxResponseBytes <= 0 || maxResponseBytes == math.MaxInt64 {
		return nil, fmt.Errorf("invalid OpenCost client limits")
	}
	return &openCostClient{
		baseURL:          baseURL,
		timeout:          timeout,
		maxResponseBytes: maxResponseBytes,
		httpClient: &http.Client{
			Timeout: timeout,
			CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
				return http.ErrUseLastResponse
			},
		},
	}, nil
}

func (c *openCostClient) Allocations(ctx context.Context, start, end time.Time, step time.Duration, namespaces []string) ([]Allocation, time.Time, bool, error) {
	if start.IsZero() || end.IsZero() || !end.After(start) || len(namespaces) == 0 {
		return nil, time.Time{}, false, fmt.Errorf("invalid OpenCost allocation query")
	}
	stepText, err := openCostStep(step)
	if err != nil {
		return nil, time.Time{}, false, err
	}
	filter, namespaces, err := namespaceFilter(namespaces)
	if err != nil {
		return nil, time.Time{}, false, err
	}

	endpoint := *c.baseURL
	endpoint.Path = strings.TrimRight(endpoint.Path, "/") + "/allocation/compute"
	values := make(url.Values)
	values.Set("window", start.UTC().Format(time.RFC3339)+","+end.UTC().Format(time.RFC3339))
	values.Set("aggregate", "cluster,namespace,pod")
	values.Set("includeIdle", "false")
	values.Set("includeSharedCostBreakdown", "false")
	values.Set("accumulate", "false")
	values.Set("step", stepText)
	values.Set("filter", filter)
	endpoint.RawQuery = values.Encode()

	requestCtx, cancel := context.WithTimeout(ctx, c.timeout)
	defer cancel()
	request, err := http.NewRequestWithContext(requestCtx, http.MethodGet, endpoint.String(), nil)
	if err != nil {
		return nil, time.Time{}, false, fmt.Errorf("build OpenCost request: %w", err)
	}
	response, err := c.httpClient.Do(request)
	if err != nil {
		return nil, time.Time{}, false, fmt.Errorf("request OpenCost allocations: %w", err)
	}
	defer response.Body.Close()
	if response.StatusCode < http.StatusOK || response.StatusCode >= http.StatusMultipleChoices {
		return nil, time.Time{}, false, fmt.Errorf("OpenCost allocation request failed with status %d", response.StatusCode)
	}
	body, err := io.ReadAll(io.LimitReader(response.Body, c.maxResponseBytes+1))
	if err != nil {
		return nil, time.Time{}, false, fmt.Errorf("read OpenCost allocation response: %w", err)
	}
	if int64(len(body)) > c.maxResponseBytes {
		return nil, time.Time{}, false, fmt.Errorf("OpenCost allocation response exceeds limit")
	}
	allocations, asOf, incomplete, err := parseOpenCostAllocations(body, start, end, step, namespaces)
	if err != nil {
		return nil, time.Time{}, false, err
	}
	return allocations, asOf, incomplete, nil
}

func openCostStep(step time.Duration) (string, error) {
	switch step {
	case time.Hour:
		return "1h", nil
	case 24 * time.Hour:
		return "1d", nil
	default:
		return "", fmt.Errorf("unsupported OpenCost allocation step")
	}
}

func namespaceFilter(input []string) (string, []string, error) {
	seen := make(map[string]struct{}, len(input))
	namespaces := make([]string, 0, len(input))
	for _, namespace := range input {
		if len(namespace) > 63 || !dns1123Label.MatchString(namespace) {
			return "", nil, fmt.Errorf("invalid namespace")
		}
		if _, ok := seen[namespace]; !ok {
			seen[namespace] = struct{}{}
			namespaces = append(namespaces, namespace)
		}
	}
	sort.Strings(namespaces)
	parts := make([]string, len(namespaces))
	for index, namespace := range namespaces {
		parts[index] = `namespace:"` + namespace + `"`
	}
	return strings.Join(parts, "|"), namespaces, nil
}

type openCostResponse struct {
	Code     *int            `json:"code"`
	Data     json.RawMessage `json:"data"`
	Error    json.RawMessage `json:"error"`
	Warnings json.RawMessage `json:"warnings"`
}

type openCostAllocation struct {
	Properties struct {
		Namespace string `json:"namespace"`
		Pod       string `json:"pod"`
	} `json:"properties"`
	Window                 json.RawMessage `json:"window"`
	Minutes                *float64        `json:"minutes"`
	CPUUsageAverage        *float64        `json:"cpuCoreUsageAverage"`
	CPURequestAverage      *float64        `json:"cpuCoreRequestAverage"`
	RAMUsageAverageBytes   *float64        `json:"ramByteUsageAverage"`
	RAMRequestAverageBytes *float64        `json:"ramByteRequestAverage"`
}

type allocationFrame struct {
	start time.Time
	end   time.Time
}

func parseOpenCostAllocations(body []byte, requestedStart, requestedEnd time.Time, step time.Duration, namespaces []string) ([]Allocation, time.Time, bool, error) {
	var payload openCostResponse
	if err := json.Unmarshal(body, &payload); err != nil {
		return nil, time.Time{}, false, fmt.Errorf("parse OpenCost allocation response: %w", err)
	}
	if payload.Code == nil || *payload.Code != http.StatusOK || hasOpenCostError(payload.Error) {
		return nil, time.Time{}, false, fmt.Errorf("OpenCost allocation response reports an error")
	}
	if len(payload.Data) == 0 || string(payload.Data) == "null" {
		return nil, time.Time{}, false, fmt.Errorf("OpenCost allocation response has no data")
	}
	var frames []map[string]json.RawMessage
	if err := json.Unmarshal(payload.Data, &frames); err != nil {
		return nil, time.Time{}, false, newSanitizedError("OpenCost allocation response has invalid data", err)
	}
	if frames == nil {
		return nil, time.Time{}, false, fmt.Errorf("OpenCost allocation response has invalid data")
	}

	allowed := make(map[string]struct{}, len(namespaces))
	for _, namespace := range namespaces {
		allowed[namespace] = struct{}{}
	}
	incomplete := hasOpenCostWarnings(payload.Warnings)
	coverage := make([]allocationFrame, 0, len(frames))
	for _, frame := range frames {
		window, ok := openCostFrameWindow(frame, requestedStart, requestedEnd, step)
		if !ok {
			incomplete = true
			continue
		}
		coverage = append(coverage, window)
	}
	asOf, coverageComplete := completeCoverageBoundary(coverage, requestedStart, requestedEnd)
	incomplete = incomplete || !coverageComplete

	allocations := make([]Allocation, 0)
	rowCount := 0
	for _, group := range frames {
		for _, raw := range group {
			rowCount++
			if rowCount > maxAllocationRows {
				return nil, time.Time{}, false, fmt.Errorf("OpenCost allocation response exceeds row limit")
			}
			var row openCostAllocation
			if err := json.Unmarshal(raw, &row); err != nil {
				return nil, time.Time{}, false, newSanitizedError("OpenCost allocation response contains malformed allocation data", err)
			}
			allocation, ok := validateOpenCostAllocation(row, requestedStart, requestedEnd, step, allowed)
			if !ok {
				incomplete = true
				continue
			}
			if allocation.End.After(asOf) {
				continue
			}
			allocations = append(allocations, allocation)
		}
	}
	return allocations, asOf, incomplete, nil
}

func hasOpenCostError(raw json.RawMessage) bool {
	return len(raw) > 0 && string(raw) != "null" && string(raw) != `""`
}

func hasOpenCostWarnings(raw json.RawMessage) bool {
	return len(raw) > 0 && string(raw) != "null" && string(raw) != "[]"
}

func openCostFrameWindow(frame map[string]json.RawMessage, requestedStart, requestedEnd time.Time, step time.Duration) (allocationFrame, bool) {
	if len(frame) == 0 {
		return allocationFrame{}, false
	}
	var result allocationFrame
	for _, raw := range frame {
		var header struct {
			Window json.RawMessage `json:"window"`
		}
		if err := json.Unmarshal(raw, &header); err != nil {
			return allocationFrame{}, false
		}
		start, end, ok := parseOpenCostWindow(header.Window)
		if !ok || start.Before(requestedStart) || end.After(requestedEnd) || !end.After(start) || end.Sub(start) != step {
			return allocationFrame{}, false
		}
		if result.start.IsZero() {
			result = allocationFrame{start: start, end: end}
			continue
		}
		if !result.start.Equal(start) || !result.end.Equal(end) {
			return allocationFrame{}, false
		}
	}
	return result, !result.start.IsZero()
}

func completeCoverageBoundary(frames []allocationFrame, requestedStart, requestedEnd time.Time) (time.Time, bool) {
	if len(frames) == 0 {
		return requestedStart, false
	}
	sort.Slice(frames, func(i, j int) bool {
		return frames[i].start.Before(frames[j].start)
	})
	latest := requestedStart
	expected := requestedStart
	complete := true
	for _, frame := range frames {
		if !frame.start.Equal(expected) {
			complete = false
		}
		if frame.end.After(expected) {
			expected = frame.end
		}
		if frame.end.After(latest) {
			latest = frame.end
		}
	}
	if latest.After(requestedEnd) {
		latest = requestedEnd
	}
	return latest, complete && expected.Equal(requestedEnd)
}

func validateOpenCostAllocation(row openCostAllocation, requestedStart, requestedEnd time.Time, step time.Duration, allowed map[string]struct{}) (Allocation, bool) {
	if _, ok := allowed[row.Properties.Namespace]; !ok || strings.TrimSpace(row.Properties.Pod) == "" || len(row.Properties.Pod) > 253 || row.Minutes == nil || row.CPUUsageAverage == nil || row.CPURequestAverage == nil || row.RAMUsageAverageBytes == nil || row.RAMRequestAverageBytes == nil {
		return Allocation{}, false
	}
	start, end, ok := parseOpenCostWindow(row.Window)
	if !ok || start.Before(requestedStart) || end.After(requestedEnd) || !end.After(start) || end.Sub(start) != step {
		return Allocation{}, false
	}
	values := []float64{*row.Minutes, *row.CPUUsageAverage, *row.CPURequestAverage, *row.RAMUsageAverageBytes, *row.RAMRequestAverageBytes}
	for _, value := range values {
		if math.IsNaN(value) || math.IsInf(value, 0) || value < 0 || value > 1e15 {
			return Allocation{}, false
		}
	}
	if *row.Minutes > end.Sub(start).Minutes()+0.000001 {
		return Allocation{}, false
	}
	return Allocation{Start: start, End: end, Namespace: row.Properties.Namespace, Pod: row.Properties.Pod, Minutes: *row.Minutes, CPUUsageAverage: *row.CPUUsageAverage, CPURequestAverage: *row.CPURequestAverage, RAMUsageAverageBytes: *row.RAMUsageAverageBytes, RAMRequestAverageBytes: *row.RAMRequestAverageBytes}, true
}

func parseOpenCostWindow(raw json.RawMessage) (time.Time, time.Time, bool) {
	var object struct {
		Start string `json:"start"`
		End   string `json:"end"`
	}
	if json.Unmarshal(raw, &object) == nil && object.Start != "" && object.End != "" {
		start, startErr := time.Parse(time.RFC3339, object.Start)
		end, endErr := time.Parse(time.RFC3339, object.End)
		return start.UTC(), end.UTC(), startErr == nil && endErr == nil
	}
	var legacy string
	if err := json.Unmarshal(raw, &legacy); err != nil || len(legacy) < 5 || legacy[0] != '[' || legacy[len(legacy)-1] != ')' {
		return time.Time{}, time.Time{}, false
	}
	parts := strings.Split(strings.TrimSuffix(strings.TrimPrefix(legacy, "["), ")"), ",")
	if len(parts) != 2 {
		return time.Time{}, time.Time{}, false
	}
	start, startErr := time.Parse(time.RFC3339, strings.TrimSpace(parts[0]))
	end, endErr := time.Parse(time.RFC3339, strings.TrimSpace(parts[1]))
	return start.UTC(), end.UTC(), startErr == nil && endErr == nil
}

var _ AllocationClient = (*openCostClient)(nil)
