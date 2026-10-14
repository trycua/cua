package metering

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestPrometheusClientParsesMatrix(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		if request.URL.Path != "/api/v1/query_range" || request.URL.Query().Get("query") != "metric_name" {
			t.Fatalf("unexpected request: %s", request.URL.String())
		}
		response.Header().Set("Content-Type", "application/json")
		_, _ = response.Write([]byte(`{"status":"success","data":{"resultType":"matrix","result":[{"metric":{"sandbox_uid":"uid-1"},"values":[[1787382000.5,"4"]]}]}}`))
	}))
	defer server.Close()

	client, err := NewPrometheusClient(server.URL, server.Client())
	if err != nil {
		t.Fatal(err)
	}
	series, err := client.QueryRange(context.Background(), "metric_name", time.Unix(1787382000, 0), time.Unix(1787382060, 0), 15*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if len(series) != 1 || series[0].Labels["sandbox_uid"] != "uid-1" || len(series[0].Samples) != 1 || series[0].Samples[0].Value != 4 {
		t.Fatalf("series = %#v", series)
	}
	if got := series[0].Samples[0].Timestamp.Nanosecond(); got != 500000000 {
		t.Fatalf("timestamp nanos = %d, want 500000000", got)
	}
}
