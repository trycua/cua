package metering

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
)

const maxPrometheusResponseBytes = 32 << 20

type Series struct {
	Labels  map[string]string
	Samples []Sample
}

type PrometheusClient struct {
	baseURL *url.URL
	client  *http.Client
}

func NewPrometheusClient(rawURL string, client *http.Client) (*PrometheusClient, error) {
	baseURL, err := url.Parse(rawURL)
	if err != nil {
		return nil, fmt.Errorf("invalid Prometheus URL: %w", err)
	}
	if (baseURL.Scheme != "http" && baseURL.Scheme != "https") || baseURL.Host == "" {
		return nil, fmt.Errorf("invalid Prometheus URL")
	}
	if client == nil {
		client = &http.Client{Timeout: 30 * time.Second}
	}
	return &PrometheusClient{baseURL: baseURL, client: client}, nil
}

func (c *PrometheusClient) QueryRange(ctx context.Context, query string, start, end time.Time, step time.Duration) ([]Series, error) {
	if strings.TrimSpace(query) == "" || !end.After(start) || step <= 0 {
		return nil, fmt.Errorf("invalid Prometheus range query")
	}
	endpoint := *c.baseURL
	endpoint.Path = strings.TrimRight(endpoint.Path, "/") + "/api/v1/query_range"
	values := endpoint.Query()
	values.Set("query", query)
	values.Set("start", formatPrometheusTime(start))
	values.Set("end", formatPrometheusTime(end))
	values.Set("step", strconv.FormatFloat(step.Seconds(), 'f', -1, 64))
	endpoint.RawQuery = values.Encode()

	request, err := http.NewRequestWithContext(ctx, http.MethodGet, endpoint.String(), nil)
	if err != nil {
		return nil, fmt.Errorf("build Prometheus request: %w", err)
	}
	response, err := c.client.Do(request)
	if err != nil {
		return nil, fmt.Errorf("query Prometheus: %w", err)
	}
	defer response.Body.Close()
	body, err := io.ReadAll(io.LimitReader(response.Body, maxPrometheusResponseBytes+1))
	if err != nil {
		return nil, fmt.Errorf("read Prometheus response: %w", err)
	}
	if len(body) > maxPrometheusResponseBytes {
		return nil, fmt.Errorf("Prometheus response exceeds %d bytes", maxPrometheusResponseBytes)
	}
	if response.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("Prometheus returned HTTP %d", response.StatusCode)
	}

	var payload struct {
		Status string `json:"status"`
		Data   struct {
			ResultType string `json:"resultType"`
			Result     []struct {
				Metric map[string]string   `json:"metric"`
				Values [][]json.RawMessage `json:"values"`
			} `json:"result"`
		} `json:"data"`
		Error string `json:"error"`
	}
	if err := json.Unmarshal(body, &payload); err != nil {
		return nil, fmt.Errorf("decode Prometheus response: %w", err)
	}
	if payload.Status != "success" || payload.Data.ResultType != "matrix" {
		return nil, fmt.Errorf("Prometheus range query failed")
	}

	series := make([]Series, 0, len(payload.Data.Result))
	for _, result := range payload.Data.Result {
		parsed := Series{Labels: result.Metric, Samples: make([]Sample, 0, len(result.Values))}
		for _, pair := range result.Values {
			if len(pair) != 2 {
				return nil, fmt.Errorf("invalid Prometheus sample pair")
			}
			var timestamp float64
			var valueText string
			if err := json.Unmarshal(pair[0], &timestamp); err != nil {
				return nil, fmt.Errorf("decode Prometheus timestamp: %w", err)
			}
			if err := json.Unmarshal(pair[1], &valueText); err != nil {
				return nil, fmt.Errorf("decode Prometheus value: %w", err)
			}
			value, err := strconv.ParseFloat(valueText, 64)
			if err != nil {
				return nil, fmt.Errorf("parse Prometheus value: %w", err)
			}
			seconds, fraction := mathModf(timestamp)
			parsed.Samples = append(parsed.Samples, Sample{
				Timestamp: time.Unix(seconds, int64(fraction*float64(time.Second))).UTC(),
				Value:     value,
			})
		}
		series = append(series, parsed)
	}
	return series, nil
}

func formatPrometheusTime(value time.Time) string {
	return strconv.FormatFloat(float64(value.UnixNano())/float64(time.Second), 'f', 3, 64)
}

func mathModf(value float64) (int64, float64) {
	seconds := int64(value)
	return seconds, value - float64(seconds)
}
