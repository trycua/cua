package productanalytics

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestClientDeliversAllowlistedEvent(t *testing.T) {
	requests := make(chan map[string]any, 1)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var payload map[string]any
		if err := json.NewDecoder(r.Body).Decode(&payload); err != nil {
			t.Fatalf("decode payload: %v", err)
		}
		requests <- payload
		w.WriteHeader(http.StatusOK)
	}))
	defer server.Close()

	client := New(Config{
		Enabled: true, Host: server.URL, ProjectToken: "phc_test", IdentityKey: "identity-test-key", Environment: "production",
		QueueSize: 4, BatchSize: 1, FlushInterval: time.Hour, RequestTimeout: time.Second,
	})
	client.Capture(Event{
		Name: EventPoolCreate, DistinctID: "subject-1", InsertID: "trace-1",
		Properties: map[string]any{"outcome": OutcomeSuccess, "source": SourceSPA, "principal_type": "user", "status_code": 201},
	})

	select {
	case payload := <-requests:
		encoded, err := json.Marshal(payload)
		if err != nil || bytes.Contains(encoded, []byte("subject-1")) {
			t.Fatalf("payload contains raw subject or failed to encode: %s, %v", encoded, err)
		}
		if payload["api_key"] != "phc_test" {
			t.Fatalf("api_key = %#v", payload["api_key"])
		}
		batch := payload["batch"].([]any)
		item := batch[0].(map[string]any)
		if item["event"] != EventPoolCreate || item["distinct_id"] != PseudonymForUserID("subject-1", "identity-test-key") {
			t.Fatalf("event payload = %#v", item)
		}
		properties := item["properties"].(map[string]any)
		if properties["environment"] != "production" || properties["instrumentation_version"] != Version || properties["$insert_id"] != "trace-1" {
			t.Fatalf("properties = %#v", properties)
		}
		if _, ok := properties["$set_once"].(map[string]any)["fleet_first_seen_at"]; !ok {
			t.Fatalf("$set_once = %#v", properties["$set_once"])
		}
	case <-time.After(2 * time.Second):
		t.Fatal("timed out waiting for capture")
	}

	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if err := client.Shutdown(ctx); err != nil {
		t.Fatalf("Shutdown() error = %v", err)
	}
}

func TestClientSuppressesExcludedSubject(t *testing.T) {
	requests := make(chan struct{}, 1)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		requests <- struct{}{}
		w.WriteHeader(http.StatusOK)
	}))
	defer server.Close()

	client := New(Config{
		Enabled: true, Host: server.URL, ProjectToken: "phc_test", IdentityKey: "identity-test-key", Environment: "production",
		ExcludedSubjects: []string{"internal-1"}, QueueSize: 1, BatchSize: 1,
		FlushInterval: 10 * time.Millisecond, RequestTimeout: time.Second,
	})
	client.Capture(Event{Name: EventLoginSucceeded, DistinctID: "internal-1", Properties: map[string]any{"outcome": OutcomeSuccess, "source": SourceSPA, "principal_type": "user"}})
	time.Sleep(30 * time.Millisecond)
	select {
	case <-requests:
		t.Fatal("excluded subject was delivered")
	default:
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	_ = client.Shutdown(ctx)
}

func TestClientPseudonymizesAttributionIdentityAndStableInsertID(t *testing.T) {
	requests := make(chan map[string]any, 1)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var payload map[string]any
		if err := json.NewDecoder(r.Body).Decode(&payload); err != nil {
			t.Fatal(err)
		}
		requests <- payload
		w.WriteHeader(http.StatusOK)
	}))
	defer server.Close()
	client := New(Config{
		Enabled: true, Host: server.URL, ProjectToken: "phc_test", IdentityKey: "identity-test-key", Environment: "production",
		QueueSize: 2, BatchSize: 1, FlushInterval: time.Hour, RequestTimeout: time.Second,
	})
	client.Capture(Event{
		Name: EventAttributionBound, DistinctID: "subject-1",
		Properties: map[string]any{"outcome": OutcomeSuccess, "source": SourceSPA, "principal_type": "user", "identity_class": IdentityExternal},
		SetOnce:    map[string]any{FirstTouchUTMCampaignProperty: "openclaw-2-launch"},
	})
	select {
	case payload := <-requests:
		encoded, _ := json.Marshal(payload)
		if bytes.Contains(encoded, []byte("subject-1")) {
			t.Fatalf("payload contains raw subject: %s", encoded)
		}
		item := payload["batch"].([]any)[0].(map[string]any)
		pseudonym := PseudonymForUserID("subject-1", "identity-test-key")
		properties := item["properties"].(map[string]any)
		if item["distinct_id"] != pseudonym || properties["$insert_id"] != "fleet-attribution:"+pseudonym {
			t.Fatalf("item = %#v", item)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("timed out waiting for attribution capture")
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if err := client.Shutdown(ctx); err != nil {
		t.Fatal(err)
	}
}
