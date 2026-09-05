package usage

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"
)

type fakeQueryObjectStore struct {
	mu          sync.Mutex
	status      [][]byte
	result      []byte
	deletedKeys []string
}

func (store *fakeQueryObjectStore) PresignPut(_ context.Context, _ string, key, contentType string, _ time.Duration) (string, error) {
	return "https://uploads.example.test/" + key + "?X-Amz-Signature=secret-" + contentType, nil
}

func (store *fakeQueryObjectStore) Get(_ context.Context, _ string, key string, _ int64) ([]byte, error) {
	store.mu.Lock()
	defer store.mu.Unlock()
	if strings.HasSuffix(key, "/status.json") {
		if len(store.status) == 0 {
			return nil, ErrQueryObjectNotFound
		}
		body := store.status[0]
		store.status = store.status[1:]
		return body, nil
	}
	if strings.HasSuffix(key, "/result.csv") {
		return store.result, nil
	}
	return nil, ErrQueryObjectNotFound
}

func (store *fakeQueryObjectStore) Delete(_ context.Context, _ string, key string) error {
	store.mu.Lock()
	defer store.mu.Unlock()
	store.deletedKeys = append(store.deletedKeys, key)
	return nil
}

func TestDataFusionAllocationClientQueriesBoundedParquetAndMapsSemantics(t *testing.T) {
	t.Parallel()
	secret := "webhook-secret"
	var payload dataFusionWebhookPayload
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		body, err := io.ReadAll(request.Body)
		if err != nil {
			t.Fatal(err)
		}
		mac := hmac.New(sha256.New, []byte(secret))
		_, _ = mac.Write(body)
		wantSignature := "sha256=" + hex.EncodeToString(mac.Sum(nil))
		if request.Header.Get("X-Hub-Signature-256") != wantSignature {
			t.Fatalf("signature = %q, want %q", request.Header.Get("X-Hub-Signature-256"), wantSignature)
		}
		if err := json.Unmarshal(body, &payload); err != nil {
			t.Fatal(err)
		}
		writer.WriteHeader(http.StatusAccepted)
	}))
	defer server.Close()

	start := time.Date(2026, 8, 19, 9, 0, 0, 0, time.UTC)
	store := &fakeQueryObjectStore{
		status: [][]byte{[]byte(`{"status":"succeeded","rows":1,"partitions_expanded":1,"partitions_matched":1,"partitions_missing":0}`)},
		result: []byte("window_start,window_end,namespace,pod,cpu_usage_core_hours,cpu_request_core_hours,ram_usage_byte_hours,ram_request_byte_hours,total_cost_usd\n" +
			"2026-08-19 09:00:00,2026-08-19 10:00:00,ns-a,virt-launcher-vm-a-x,2,4,3221225472,6442450944,1.25\n"),
	}
	client, err := NewDataFusionAllocationClient(server.URL, secret, "results-bucket", "cyclops/usage-query", "kopf-k3s", "production", time.Second, time.Millisecond, 1<<20, store)
	if err != nil {
		t.Fatal(err)
	}
	allocations, asOf, partial, err := client.Allocations(context.Background(), start, start.Add(time.Hour), time.Hour, []string{"ns-b", "ns-a", "ns-a"})
	if err != nil || partial || !asOf.Equal(start.Add(time.Hour)) || len(allocations) != 1 {
		t.Fatalf("allocations=%#v asOf=%s partial=%t err=%v", allocations, asOf, partial, err)
	}
	allocation := allocations[0]
	if allocation.Minutes != 60 || allocation.CPUUsageAverage != 2 || allocation.CPURequestAverage != 4 || allocation.RAMUsageAverageBytes != 3*gibibyte || allocation.RAMRequestAverageBytes != 6*gibibyte || allocation.CostUSD != 1.25 {
		t.Fatalf("allocation = %#v", allocation)
	}
	if payload.Dataset != "allocation" || payload.Cluster != "kopf-k3s" || payload.Environment != "production" || payload.SchemaVersion != "v2" || payload.OutputFormat != "csv" {
		t.Fatalf("payload = %#v", payload)
	}
	for _, fragment := range []string{"namespace IN ('ns-a','ns-b')", "LIMIT 100001", "cpu_usage_core_hours", "cpu_request_core_hours", "ram_usage_byte_hours", "ram_request_byte_hours", "total_cost_usd"} {
		if !strings.Contains(payload.Query, fragment) {
			t.Fatalf("query %q does not contain %q", payload.Query, fragment)
		}
	}
	if len(store.deletedKeys) != 2 {
		t.Fatalf("deleted keys = %v", store.deletedKeys)
	}
}

func TestDataFusionAllocationClientRejectsUnsafeNamespaceBeforeWebhook(t *testing.T) {
	t.Parallel()
	called := false
	server := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) { called = true }))
	defer server.Close()
	client, err := NewDataFusionAllocationClient(server.URL, "secret", "bucket", "cyclops/usage-query", "kopf-k3s", "production", time.Second, time.Millisecond, 1<<20, &fakeQueryObjectStore{})
	if err != nil {
		t.Fatal(err)
	}
	start := time.Date(2026, 8, 19, 9, 0, 0, 0, time.UTC)
	_, _, _, err = client.Allocations(context.Background(), start, start.Add(time.Hour), time.Hour, []string{"ns-a') OR true --"})
	if err == nil || !strings.Contains(err.Error(), "invalid namespace") || called {
		t.Fatalf("called=%t err=%v", called, err)
	}
}

func TestDataFusionAllocationClientHandlesMalformedNon2xxAndTimeoutSafely(t *testing.T) {
	t.Parallel()
	start := time.Date(2026, 8, 19, 9, 0, 0, 0, time.UTC)
	for name, test := range map[string]struct {
		handler http.HandlerFunc
		store   *fakeQueryObjectStore
		want    string
	}{
		"non_2xx": {
			handler: func(writer http.ResponseWriter, _ *http.Request) {
				http.Error(writer, "SELECT secret FROM tenant", http.StatusBadGateway)
			},
			store: &fakeQueryObjectStore{},
			want:  "query webhook returned HTTP 502",
		},
		"malformed_csv": {
			handler: func(writer http.ResponseWriter, _ *http.Request) { writer.WriteHeader(http.StatusAccepted) },
			store: &fakeQueryObjectStore{
				status: [][]byte{[]byte(`{"status":"succeeded","rows":1,"partitions_expanded":1,"partitions_matched":1}`)},
				result: []byte("window_start,namespace\nhttps://bucket.example/?X-Amz-Signature=leak,ns-a\n"),
			},
			want: "decode allocation query result",
		},
		"timeout": {
			handler: func(writer http.ResponseWriter, _ *http.Request) { writer.WriteHeader(http.StatusAccepted) },
			store:   &fakeQueryObjectStore{},
			want:    "allocation query timed out",
		},
	} {
		t.Run(name, func(t *testing.T) {
			server := httptest.NewServer(test.handler)
			defer server.Close()
			timeout := time.Second
			if name == "timeout" {
				timeout = 5 * time.Millisecond
			}
			client, err := NewDataFusionAllocationClient(server.URL, "secret", "bucket", "cyclops/usage-query", "kopf-k3s", "production", timeout, time.Millisecond, 1<<20, test.store)
			if err != nil {
				t.Fatal(err)
			}
			_, _, _, err = client.Allocations(context.Background(), start, start.Add(time.Hour), time.Hour, []string{"ns-a"})
			if err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v, want containing %q", err, test.want)
			}
			for _, forbidden := range []string{"SELECT secret", "X-Amz-Signature", "tenant"} {
				if strings.Contains(err.Error(), forbidden) {
					t.Fatalf("error leaked %q: %v", forbidden, err)
				}
			}
		})
	}
}

func TestDataFusionAllocationClientPreservesCancellationAndMissingPartitions(t *testing.T) {
	t.Parallel()
	start := time.Date(2026, 8, 19, 9, 0, 0, 0, time.UTC)
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, _ *http.Request) { writer.WriteHeader(http.StatusAccepted) }))
	defer server.Close()
	store := &fakeQueryObjectStore{
		status: [][]byte{[]byte(`{"status":"succeeded","rows":0,"partitions_expanded":2,"partitions_matched":1,"partitions_missing":1}`)},
		result: []byte("window_start,window_end,namespace,pod,cpu_usage_core_hours,cpu_request_core_hours,ram_usage_byte_hours,ram_request_byte_hours,total_cost_usd\n"),
	}
	client, err := NewDataFusionAllocationClient(server.URL, "secret", "bucket", "cyclops/usage-query", "kopf-k3s", "production", time.Second, time.Millisecond, 1<<20, store)
	if err != nil {
		t.Fatal(err)
	}
	allocations, asOf, partial, err := client.Allocations(context.Background(), start, start.Add(2*time.Hour), time.Hour, []string{"ns-a"})
	if err != nil || len(allocations) != 0 || !partial || !asOf.Equal(start) {
		t.Fatalf("allocations=%v asOf=%s partial=%t err=%v", allocations, asOf, partial, err)
	}

	cancelled, cancel := context.WithCancel(context.Background())
	cancel()
	_, _, _, err = client.Allocations(cancelled, start, start.Add(time.Hour), time.Hour, []string{"ns-a"})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("error = %v, want context.Canceled", err)
	}
}

func TestDataFusionAllocationClientMarksLegacyParquetRowsPartial(t *testing.T) {
	t.Parallel()
	start := time.Date(2026, 8, 19, 9, 0, 0, 0, time.UTC)
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, _ *http.Request) {
		writer.WriteHeader(http.StatusAccepted)
	}))
	defer server.Close()
	store := &fakeQueryObjectStore{
		status: [][]byte{[]byte(`{"status":"succeeded","table":"allocation","rows":1,"partitions_expanded":1,"partitions_matched":1,"partitions_missing":0}`)},
		result: []byte("window_start,window_end,namespace,pod,cpu_usage_core_hours,cpu_request_core_hours,ram_usage_byte_hours,ram_request_byte_hours,total_cost_usd\n" +
			"2026-08-19T09:00:00Z,2026-08-19T10:00:00Z,ns-a,NULL,NULL,NULL,NULL,NULL,NULL\n"),
	}
	client, err := NewDataFusionAllocationClient(server.URL, "secret", "bucket", "cyclops/usage-query", "kopf-k3s", "production", time.Second, time.Millisecond, 1<<20, store)
	if err != nil {
		t.Fatal(err)
	}
	allocations, asOf, partial, err := client.Allocations(context.Background(), start, start.Add(time.Hour), time.Hour, []string{"ns-a"})
	if err != nil || len(allocations) != 0 || !partial || !asOf.Equal(start) {
		t.Fatalf("allocations=%v asOf=%s partial=%t err=%v", allocations, asOf, partial, err)
	}
}
