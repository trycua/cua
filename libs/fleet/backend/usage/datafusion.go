package usage

import (
	"bytes"
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/csv"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"net/http"
	"net/url"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/google/uuid"
)

const (
	maxDataFusionAllocationRows = 100000
	maxAllocationPartitions     = 2000
	maxAllocationNamespaces     = 500
	maxAllocationQueryBytes     = 65536
	queryPresignGrace           = 10 * time.Minute
	queryCleanupTimeout         = 5 * time.Second
)

var (
	ErrQueryObjectNotFound = errors.New("query object not found")
	queryPartitionValue    = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]*$`)
	queryPodName           = regexp.MustCompile(`^[a-z0-9](?:[-a-z0-9.]*[a-z0-9])?$`)
)

type QueryObjectStore interface {
	PresignPut(context.Context, string, string, string, time.Duration) (string, error)
	Get(context.Context, string, string, int64) ([]byte, error)
	Delete(context.Context, string, string) error
}

type dataFusionAllocationClient struct {
	webhookURL      *url.URL
	hmacSecret      string
	resultBucket    string
	resultPrefix    string
	cluster         string
	environment     string
	timeout         time.Duration
	pollInterval    time.Duration
	maxResponseSize int64
	store           QueryObjectStore
	httpClient      *http.Client
}

type dataFusionWebhookPayload struct {
	Dataset       string                `json:"dataset"`
	Cluster       string                `json:"cluster"`
	Environment   string                `json:"environment"`
	SchemaVersion string                `json:"schema_version"`
	Window        dataFusionQueryWindow `json:"window"`
	Query         string                `json:"query"`
	OutputURL     string                `json:"output_url"`
	OutputFormat  string                `json:"output_format"`
	StatusURL     string                `json:"status_url"`
}

type dataFusionQueryWindow struct {
	Start time.Time `json:"start"`
	End   time.Time `json:"end"`
}

type dataFusionQueryStatus struct {
	Status             string `json:"status"`
	Rows               int64  `json:"rows"`
	PartitionsExpanded int    `json:"partitions_expanded"`
	PartitionsMatched  int    `json:"partitions_matched"`
	PartitionsMissing  int    `json:"partitions_missing"`
}

func NewDataFusionAllocationClient(rawURL, hmacSecret, resultBucket, resultPrefix, cluster, environment string, timeout, pollInterval time.Duration, maxResponseBytes int64, store QueryObjectStore) (*dataFusionAllocationClient, error) {
	webhookURL, err := url.Parse(strings.TrimSpace(rawURL))
	if err != nil {
		return nil, newSanitizedError("invalid allocation query webhook URL", err)
	}
	if (webhookURL.Scheme != "http" && webhookURL.Scheme != "https") || webhookURL.Host == "" || webhookURL.User != nil || webhookURL.RawQuery != "" || webhookURL.Fragment != "" {
		return nil, fmt.Errorf("invalid allocation query webhook URL")
	}
	resultPrefix = strings.Trim(strings.TrimSpace(resultPrefix), "/")
	if strings.TrimSpace(hmacSecret) == "" || strings.TrimSpace(resultBucket) == "" || !safeResultPrefix(resultPrefix) || !queryPartitionValue.MatchString(cluster) || !queryPartitionValue.MatchString(environment) || timeout <= 0 || pollInterval <= 0 || maxResponseBytes <= 0 || maxResponseBytes == math.MaxInt64 || store == nil {
		return nil, fmt.Errorf("invalid DataFusion allocation client configuration")
	}
	return &dataFusionAllocationClient{
		webhookURL:      webhookURL,
		hmacSecret:      hmacSecret,
		resultBucket:    strings.TrimSpace(resultBucket),
		resultPrefix:    resultPrefix,
		cluster:         cluster,
		environment:     environment,
		timeout:         timeout,
		pollInterval:    pollInterval,
		maxResponseSize: maxResponseBytes,
		store:           store,
		httpClient: &http.Client{
			CheckRedirect: func(_ *http.Request, _ []*http.Request) error { return http.ErrUseLastResponse },
		},
	}, nil
}

func safeResultPrefix(prefix string) bool {
	if prefix == "" || strings.Contains(prefix, "\\") {
		return false
	}
	for _, part := range strings.Split(prefix, "/") {
		if part == "" || part == "." || part == ".." || !queryPartitionValue.MatchString(part) {
			return false
		}
	}
	return true
}

func (client *dataFusionAllocationClient) Allocations(ctx context.Context, start, end time.Time, step time.Duration, namespaces []string) ([]Allocation, time.Time, bool, error) {
	if err := ctx.Err(); err != nil {
		return nil, time.Time{}, false, err
	}
	if start.IsZero() || end.IsZero() || !end.After(start) || start.UTC() != start || end.UTC() != end || !start.Equal(start.Truncate(time.Hour)) || !end.Equal(end.Truncate(time.Hour)) || len(namespaces) == 0 {
		return nil, time.Time{}, false, fmt.Errorf("invalid DataFusion allocation query")
	}
	if step != time.Hour && step != 24*time.Hour {
		return nil, time.Time{}, false, fmt.Errorf("unsupported allocation step")
	}
	if end.Sub(start) > maxAllocationPartitions*time.Hour {
		return nil, time.Time{}, false, fmt.Errorf("allocation query window exceeds limit")
	}
	namespaces, err := safeNamespaces(namespaces)
	if err != nil {
		return nil, time.Time{}, false, err
	}

	if len(namespaces) > maxAllocationNamespaces {
		return nil, time.Time{}, false, fmt.Errorf("allocation namespace limit exceeded")
	}
	query := allocationSQL(namespaces)
	if len(query) > maxAllocationQueryBytes {
		return nil, time.Time{}, false, fmt.Errorf("allocation query exceeds limit")
	}
	requestCtx, cancel := context.WithTimeout(ctx, client.timeout)
	defer cancel()

	prefix := fmt.Sprintf("%s/%s/%s", client.resultPrefix, time.Now().UTC().Format("2006-01-02"), uuid.NewString())
	resultKey := prefix + "/result.csv"
	statusKey := prefix + "/status.json"
	defer func() {
		cleanupCtx, cleanupCancel := context.WithTimeout(context.Background(), queryCleanupTimeout)
		defer cleanupCancel()
		_ = client.store.Delete(cleanupCtx, client.resultBucket, resultKey)
		_ = client.store.Delete(cleanupCtx, client.resultBucket, statusKey)
	}()

	expires := client.timeout + queryPresignGrace
	resultURL, err := client.store.PresignPut(requestCtx, client.resultBucket, resultKey, "text/csv", expires)
	if err != nil {
		return nil, time.Time{}, false, newSanitizedError("prepare allocation query result", err)
	}
	statusURL, err := client.store.PresignPut(requestCtx, client.resultBucket, statusKey, "application/json", expires)
	if err != nil {
		return nil, time.Time{}, false, newSanitizedError("prepare allocation query status", err)
	}
	payload, err := json.Marshal(dataFusionWebhookPayload{
		Dataset:       "allocation",
		Cluster:       client.cluster,
		Environment:   client.environment,
		SchemaVersion: "v2",
		Window:        dataFusionQueryWindow{Start: start, End: end},
		Query:         query,
		OutputURL:     resultURL,
		OutputFormat:  "csv",
		StatusURL:     statusURL,
	})
	if err != nil {
		return nil, time.Time{}, false, newSanitizedError("encode allocation query request", err)
	}
	request, err := http.NewRequestWithContext(requestCtx, http.MethodPost, client.webhookURL.String(), bytes.NewReader(payload))
	if err != nil {
		return nil, time.Time{}, false, newSanitizedError("build allocation query request", err)
	}
	request.Header.Set("Content-Type", "application/json")
	request.Header.Set("X-Hub-Signature-256", signQueryPayload(payload, client.hmacSecret))
	response, err := client.httpClient.Do(request)
	if err != nil {
		safeErr := newSanitizedError("send allocation query request", err)
		if errors.Is(requestCtx.Err(), context.DeadlineExceeded) {
			return nil, time.Time{}, false, errors.Join(errors.New("allocation query timed out"), context.DeadlineExceeded, safeErr)
		}
		if contextErr := requestCtx.Err(); contextErr != nil {
			return nil, time.Time{}, false, errors.Join(contextErr, safeErr)
		}
		return nil, time.Time{}, false, safeErr
	}
	if response == nil {
		return nil, time.Time{}, false, fmt.Errorf("allocation query webhook returned no response")
	}
	drainAndClose(response.Body)
	if response.StatusCode < http.StatusOK || response.StatusCode >= http.StatusMultipleChoices {
		return nil, time.Time{}, false, fmt.Errorf("allocation query webhook returned HTTP %d", response.StatusCode)
	}

	status, err := client.waitForQuery(requestCtx, statusKey)
	if err != nil {
		return nil, time.Time{}, false, err
	}
	if status.Rows > maxDataFusionAllocationRows {
		return nil, time.Time{}, false, fmt.Errorf("allocation query row limit exceeded")
	}
	contents, err := client.store.Get(requestCtx, client.resultBucket, resultKey, client.maxResponseSize)
	if err != nil {
		return nil, time.Time{}, false, newSanitizedError("read allocation query result", err)
	}
	allocations, asOf, parsedRows, legacyPartial, err := parseDataFusionAllocations(contents, start, end, namespaces)
	if err != nil {
		return nil, time.Time{}, false, err
	}
	if int64(parsedRows) != status.Rows {
		return nil, time.Time{}, false, fmt.Errorf("allocation query row count mismatch")
	}
	partial := legacyPartial || status.PartitionsMissing > 0 || status.PartitionsMatched != status.PartitionsExpanded
	if !partial {
		asOf = end
	} else if asOf.IsZero() {
		asOf = start
	}
	return allocations, asOf, partial, nil
}

func safeNamespaces(input []string) ([]string, error) {
	seen := make(map[string]struct{}, len(input))
	result := make([]string, 0, len(input))
	for _, namespace := range input {
		if len(namespace) > 63 || !dns1123Label.MatchString(namespace) {
			return nil, fmt.Errorf("invalid namespace")
		}
		if _, ok := seen[namespace]; ok {
			continue
		}
		seen[namespace] = struct{}{}
		result = append(result, namespace)
	}
	sort.Strings(result)
	return result, nil
}

func allocationSQL(namespaces []string) string {
	literals := make([]string, len(namespaces))
	for index, namespace := range namespaces {
		literals[index] = "'" + namespace + "'"
	}
	return "SELECT window_start, window_end, namespace, pod, cpu_usage_core_hours, cpu_request_core_hours, ram_usage_byte_hours, ram_request_byte_hours, total_cost_usd " +
		"FROM allocation WHERE namespace IN (" + strings.Join(literals, ",") + ") " +
		"ORDER BY window_start, namespace, pod LIMIT 100001"
}

func signQueryPayload(body []byte, secret string) string {
	mac := hmac.New(sha256.New, []byte(secret))
	_, _ = mac.Write(body)
	return "sha256=" + hex.EncodeToString(mac.Sum(nil))
}

func drainAndClose(body io.ReadCloser) {
	if body == nil {
		return
	}
	_, _ = io.Copy(io.Discard, io.LimitReader(body, 64<<10))
	_ = body.Close()
}

func (client *dataFusionAllocationClient) waitForQuery(ctx context.Context, statusKey string) (dataFusionQueryStatus, error) {
	ticker := time.NewTicker(client.pollInterval)
	defer ticker.Stop()
	for {
		status, ready, err := client.readQueryStatus(ctx, statusKey)
		if err != nil {
			return dataFusionQueryStatus{}, err
		}
		if ready {
			return status, nil
		}
		select {
		case <-ctx.Done():
			if errors.Is(ctx.Err(), context.DeadlineExceeded) {
				return dataFusionQueryStatus{}, errors.Join(errors.New("allocation query timed out"), context.DeadlineExceeded)
			}
			return dataFusionQueryStatus{}, ctx.Err()
		case <-ticker.C:
		}
	}
}

func (client *dataFusionAllocationClient) readQueryStatus(ctx context.Context, statusKey string) (dataFusionQueryStatus, bool, error) {
	body, err := client.store.Get(ctx, client.resultBucket, statusKey, client.maxResponseSize)
	if err != nil {
		if errors.Is(err, ErrQueryObjectNotFound) {
			return dataFusionQueryStatus{}, false, nil
		}
		return dataFusionQueryStatus{}, false, newSanitizedError("read allocation query status", err)
	}
	var status dataFusionQueryStatus
	if decodeErr := json.Unmarshal(body, &status); decodeErr != nil {
		return dataFusionQueryStatus{}, false, newSanitizedError("decode allocation query status", decodeErr)
	}
	switch status.Status {
	case "succeeded":
		if status.Rows < 0 || status.PartitionsExpanded < 0 || status.PartitionsMatched < 0 || status.PartitionsMissing < 0 || status.PartitionsMatched+status.PartitionsMissing != status.PartitionsExpanded {
			return dataFusionQueryStatus{}, false, fmt.Errorf("invalid allocation query status")
		}
		return status, true, nil
	case "failed":
		return dataFusionQueryStatus{}, false, fmt.Errorf("allocation query failed")
	case "running", "":
		return dataFusionQueryStatus{}, false, nil
	default:
		return dataFusionQueryStatus{}, false, fmt.Errorf("invalid allocation query status")
	}
}

func parseDataFusionAllocations(contents []byte, requestedStart, requestedEnd time.Time, namespaces []string) ([]Allocation, time.Time, int, bool, error) {
	reader := csv.NewReader(bytes.NewReader(contents))
	reader.FieldsPerRecord = -1
	header, err := reader.Read()
	if err != nil {
		return nil, time.Time{}, 0, false, newSanitizedError("decode allocation query result", err)
	}
	wantHeader := []string{"window_start", "window_end", "namespace", "pod", "cpu_usage_core_hours", "cpu_request_core_hours", "ram_usage_byte_hours", "ram_request_byte_hours", "total_cost_usd"}
	if len(header) != len(wantHeader) {
		return nil, time.Time{}, 0, false, fmt.Errorf("decode allocation query result: unexpected columns")
	}
	for index := range header {
		if header[index] != wantHeader[index] {
			return nil, time.Time{}, 0, false, fmt.Errorf("decode allocation query result: unexpected columns")
		}
	}
	allowed := make(map[string]struct{}, len(namespaces))
	for _, namespace := range namespaces {
		allowed[namespace] = struct{}{}
	}
	allocations := make([]Allocation, 0)
	var asOf time.Time
	parsedRows := 0
	legacyPartial := false
	for rowNumber := 2; ; rowNumber++ {
		record, readErr := reader.Read()
		if readErr == io.EOF {
			break
		}
		if readErr != nil {
			return nil, time.Time{}, 0, false, newSanitizedError("decode allocation query result: malformed row", readErr)
		}
		if len(record) != len(wantHeader) {
			return nil, time.Time{}, 0, false, fmt.Errorf("decode allocation query result: malformed row")
		}
		parsedRows++
		if parsedRows > maxDataFusionAllocationRows {
			return nil, time.Time{}, 0, false, fmt.Errorf("allocation query row limit exceeded")
		}
		if record[3] == "NULL" || record[4] == "NULL" || record[5] == "NULL" || record[6] == "NULL" || record[7] == "NULL" || record[8] == "NULL" {
			legacyPartial = true
			continue
		}
		allocation, parseErr := parseDataFusionAllocation(record, requestedStart, requestedEnd, allowed)
		if parseErr != nil {
			return nil, time.Time{}, 0, false, newSanitizedError(fmt.Sprintf("decode allocation query result: malformed row %d", rowNumber), parseErr)
		}
		allocations = append(allocations, allocation)
		if allocation.End.After(asOf) {
			asOf = allocation.End
		}
	}
	return allocations, asOf, parsedRows, legacyPartial, nil
}

func parseDataFusionAllocation(record []string, requestedStart, requestedEnd time.Time, allowed map[string]struct{}) (Allocation, error) {
	start, err := parseDataFusionTimestamp(record[0])
	if err != nil {
		return Allocation{}, err
	}
	end, err := parseDataFusionTimestamp(record[1])
	if err != nil {
		return Allocation{}, newSanitizedError("invalid allocation window", err)
	}
	if !end.After(start) || end.Sub(start) != time.Hour || start.Before(requestedStart) || end.After(requestedEnd) {
		return Allocation{}, fmt.Errorf("invalid allocation window")
	}
	namespace := record[2]
	if _, ok := allowed[namespace]; !ok {
		return Allocation{}, fmt.Errorf("unexpected namespace")
	}
	pod := record[3]
	if len(pod) > 253 || !queryPodName.MatchString(pod) {
		return Allocation{}, fmt.Errorf("invalid pod")
	}
	values := make([]float64, 5)
	for index := range values {
		value, parseErr := strconv.ParseFloat(record[index+4], 64)
		if parseErr != nil {
			return Allocation{}, newSanitizedError("invalid allocation value", parseErr)
		}
		if math.IsNaN(value) || math.IsInf(value, 0) || value < 0 || value > 1e18 {
			return Allocation{}, fmt.Errorf("invalid allocation value")
		}
		values[index] = value
	}
	hours := end.Sub(start).Hours()
	return Allocation{
		Start:                  start.UTC(),
		End:                    end.UTC(),
		Namespace:              namespace,
		Pod:                    pod,
		Minutes:                end.Sub(start).Minutes(),
		CPUUsageAverage:        values[0] / hours,
		CPURequestAverage:      values[1] / hours,
		RAMUsageAverageBytes:   values[2] / hours,
		RAMRequestAverageBytes: values[3] / hours,
		CostUSD:                values[4],
	}, nil
}

func parseDataFusionTimestamp(raw string) (time.Time, error) {
	parsed, err := time.Parse(time.RFC3339Nano, raw)
	if err == nil {
		return parsed.UTC(), nil
	}
	parsed, fallbackErr := time.ParseInLocation("2006-01-02 15:04:05.999999999", raw, time.UTC)
	if fallbackErr != nil {
		return time.Time{}, errors.Join(err, fallbackErr)
	}
	return parsed, nil
}

var _ AllocationClient = (*dataFusionAllocationClient)(nil)
