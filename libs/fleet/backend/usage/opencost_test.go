package usage

import (
	"context"
	"fmt"
	"math"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestOpenCostClientUsesFixedScopedAllocationQuery(t *testing.T) {
	t.Parallel()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/allocation/compute" {
			t.Fatalf("path = %q", r.URL.Path)
		}
		q := r.URL.Query()
		if q.Get("aggregate") != "cluster,namespace,pod" || q.Get("includeIdle") != "false" || q.Get("includeSharedCostBreakdown") != "false" || q.Get("accumulate") != "false" || q.Get("step") != "1h" || q.Get("filter") != `namespace:"ns-a"|namespace:"ns-b"` || q.Has("filterNamespaces") {
			t.Fatalf("query = %#v", q)
		}
		_, _ = w.Write([]byte(`{"code":200,"data":[{"cluster/ns-a/virt-launcher-vm-a-x":{"properties":{"namespace":"ns-a","pod":"virt-launcher-vm-a-x"},"window":{"start":"2026-08-19T09:00:00Z","end":"2026-08-19T10:00:00Z"},"minutes":60,"cpuCoreUsageAverage":2,"cpuCoreRequestAverage":4,"ramByteUsageAverage":3221225472,"ramByteRequestAverage":6442450944}}],"meta":{"timeGenerated":"2026-08-19T10:01:00Z"}}`))
	}))
	defer server.Close()
	client, err := NewOpenCostClient(server.URL, time.Second, 1<<20)
	if err != nil {
		t.Fatal(err)
	}
	start := time.Date(2026, 8, 19, 9, 0, 0, 0, time.UTC)
	allocations, asOf, partial, err := client.Allocations(context.Background(), start, start.Add(time.Hour), time.Hour, []string{"ns-b", "ns-a"})
	if err != nil || partial || !asOf.Equal(start.Add(time.Hour)) || len(allocations) != 1 {
		t.Fatalf("allocations=%#v asOf=%s partial=%t err=%v", allocations, asOf, partial, err)
	}
	if allocations[0].CPUUsageAverage != 2 || allocations[0].CPURequestAverage != 4 || allocations[0].RAMUsageAverageBytes != 3*gibibyte || allocations[0].RAMRequestAverageBytes != 6*gibibyte {
		t.Fatalf("allocation = %#v", allocations[0])
	}
}

func TestParseOpenCostAllocationsUsesProductionWindowShapeAndCoverage(t *testing.T) {
	t.Parallel()
	start := time.Date(2026, 7, 13, 3, 0, 0, 0, time.UTC)
	body := []byte(`{"code":200,"data":[{"default/api":{"properties":{"namespace":"default","pod":"virt-launcher-api-x"},"window":{"start":"2026-07-13T03:00:00Z","end":"2026-07-13T04:00:00Z"},"minutes":60,"cpuCoreUsageAverage":0,"cpuCoreRequestAverage":2,"ramByteUsageAverage":0,"ramByteRequestAverage":1073741824}}]}`)
	allocations, asOf, partial, err := parseOpenCostAllocations(body, start, start.Add(time.Hour), time.Hour, []string{"default"})
	if err != nil || partial || !asOf.Equal(start.Add(time.Hour)) || len(allocations) != 1 {
		t.Fatalf("allocations=%#v asOf=%s partial=%t err=%v", allocations, asOf, partial, err)
	}
	if allocations[0].CPUUsageAverage != 0 || allocations[0].RAMUsageAverageBytes != 0 {
		t.Fatalf("zero averages were not retained: %#v", allocations[0])
	}
}

func TestParseOpenCostAllocationsMarksIncompleteCoverage(t *testing.T) {
	t.Parallel()
	start := time.Date(2026, 8, 19, 9, 0, 0, 0, time.UTC)
	row := func(windowStart, windowEnd time.Time) string {
		return fmt.Sprintf(`{"properties":{"namespace":"ns-a","pod":"virt-launcher-vm-a-x"},"window":{"start":"%s","end":"%s"},"minutes":60,"cpuCoreUsageAverage":1,"cpuCoreRequestAverage":2,"ramByteUsageAverage":1073741824,"ramByteRequestAverage":2147483648}`, windowStart.Format(time.RFC3339), windowEnd.Format(time.RFC3339))
	}
	for name, test := range map[string]struct {
		body       []byte
		wantCutoff time.Time
		wantRows   int
	}{
		"empty":         {[]byte(`{"code":200,"data":[]}`), start, 0},
		"missing_frame": {[]byte(`{"code":200,"data":[{"first":` + row(start, start.Add(time.Hour)) + `},{}]}`), start.Add(time.Hour), 1},
		"gap":           {[]byte(`{"code":200,"data":[{"first":` + row(start, start.Add(time.Hour)) + `},{"third":` + row(start.Add(2*time.Hour), start.Add(3*time.Hour)) + `}]}`), start.Add(time.Hour), 1},
	} {
		t.Run(name, func(t *testing.T) {
			allocations, asOf, partial, err := parseOpenCostAllocations(test.body, start, start.Add(3*time.Hour), time.Hour, []string{"ns-a"})
			if err != nil || !partial || !asOf.Equal(test.wantCutoff) || len(allocations) != test.wantRows {
				t.Fatalf("allocations=%#v asOf=%s partial=%t err=%v", allocations, asOf, partial, err)
			}
		})
	}
}

func TestParseOpenCostAllocationsRejectsSourceErrorsAndMalformedRequiredValues(t *testing.T) {
	t.Parallel()
	start := time.Date(2026, 8, 19, 9, 0, 0, 0, time.UTC)
	for name, body := range map[string][]byte{
		"missing_code":  []byte(`{"data":[]}`),
		"error_code":    []byte(`{"code":500,"error":"upstream failed"}`),
		"success_error": []byte(`{"code":200,"error":"unexpected","data":[]}`),
	} {
		t.Run(name, func(t *testing.T) {
			if _, _, _, err := parseOpenCostAllocations(body, start, start.Add(time.Hour), time.Hour, []string{"ns-a"}); err == nil {
				t.Fatal("expected source envelope error")
			}
		})
	}
	body := []byte(`{"code":200,"data":[{"missing":{"properties":{"namespace":"ns-a","pod":"virt-launcher-vm-a-x"},"window":{"start":"2026-08-19T09:00:00Z","end":"2026-08-19T10:00:00Z"},"minutes":60,"cpuCoreUsageAverage":null,"cpuCoreRequestAverage":2,"ramByteUsageAverage":1073741824,"ramByteRequestAverage":2147483648}}]}`)
	allocations, _, partial, err := parseOpenCostAllocations(body, start, start.Add(time.Hour), time.Hour, []string{"ns-a"})
	if err != nil || !partial || len(allocations) != 0 {
		t.Fatalf("allocations=%#v partial=%t err=%v", allocations, partial, err)
	}
}

func TestOpenCostClientRejectsUnsafeNamespace(t *testing.T) {
	t.Parallel()
	client, err := NewOpenCostClient("https://opencost.invalid", time.Second, 1<<20)
	if err != nil {
		t.Fatal(err)
	}
	_, _, _, err = client.Allocations(context.Background(), time.Now().Add(-time.Hour), time.Now(), time.Hour, []string{"ns-a\"|cluster:\"all"})
	if err == nil {
		t.Fatal("expected namespace validation error")
	}
}

func TestParseOpenCostAllocationsExcludesBadRowsAsIncomplete(t *testing.T) {
	t.Parallel()
	start := time.Date(2026, 8, 19, 9, 0, 0, 0, time.UTC)
	body := []byte(`{"code":200,"data":[{"valid":{"properties":{"namespace":"ns-a","pod":"virt-launcher-vm-a-x"},"window":"[2026-08-19T09:00:00Z,2026-08-19T10:00:00Z)","minutes":60,"cpuCoreUsageAverage":1,"cpuCoreRequestAverage":2,"ramByteUsageAverage":1073741824,"ramByteRequestAverage":2147483648},"out-of-scope":{"properties":{"namespace":"other","pod":"pod"},"window":"[2026-08-19T09:00:00Z,2026-08-19T10:00:00Z)","minutes":60,"cpuCoreUsageAverage":1,"cpuCoreRequestAverage":2,"ramByteUsageAverage":1073741824,"ramByteRequestAverage":2147483648},"oversized":{"properties":{"namespace":"ns-a","pod":"pod"},"window":"[2026-08-19T09:00:00Z,2026-08-19T10:00:00Z)","minutes":60,"cpuCoreUsageAverage":1000000000000001,"cpuCoreRequestAverage":2,"ramByteUsageAverage":1073741824,"ramByteRequestAverage":2147483648}}],"warnings":["partial"]}`)
	allocations, _, partial, err := parseOpenCostAllocations(body, start, start.Add(time.Hour), time.Hour, []string{"ns-a"})
	if err != nil || !partial || len(allocations) != 1 {
		t.Fatalf("allocations=%#v partial=%t err=%v", allocations, partial, err)
	}
}

func TestOpenCostClientRejectsUnsafeBaseURLAndResponseLimitOverflow(t *testing.T) {
	t.Parallel()
	for _, rawURL := range []string{
		"https://opencost.invalid?filterNamespaces=legacy",
		"https://opencost.invalid#fragment",
		"https://user:pass@opencost.invalid",
		"ftp://opencost.invalid",
	} {
		if _, err := NewOpenCostClient(rawURL, time.Second, 1<<20); err == nil {
			t.Fatalf("expected unsafe URL rejection for %q", rawURL)
		}
	}
	if _, err := NewOpenCostClient("https://opencost.invalid", time.Second, math.MaxInt64); err == nil {
		t.Fatal("expected response limit overflow rejection")
	}
}

func TestOpenCostClientRejectsRedirectAndResponseOverLimit(t *testing.T) {
	t.Parallel()
	for name, handler := range map[string]http.HandlerFunc{
		"redirect": func(w http.ResponseWriter, r *http.Request) { http.Redirect(w, r, "/elsewhere", http.StatusFound) },
		"limit":    func(w http.ResponseWriter, r *http.Request) { _, _ = w.Write(make([]byte, 128)) },
	} {
		t.Run(name, func(t *testing.T) {
			server := httptest.NewServer(handler)
			defer server.Close()
			limit := int64(1 << 20)
			if name == "limit" {
				limit = 64
			}
			client, err := NewOpenCostClient(server.URL, time.Second, limit)
			if err != nil {
				t.Fatal(err)
			}
			_, _, _, err = client.Allocations(context.Background(), time.Now().UTC().Truncate(time.Hour), time.Now().UTC().Truncate(time.Hour).Add(time.Hour), time.Hour, []string{"ns-a"})
			if err == nil {
				t.Fatal("expected request error")
			}
		})
	}
}
