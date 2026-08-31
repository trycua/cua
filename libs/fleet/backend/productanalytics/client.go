package productanalytics

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"strings"
	"sync"
	"time"

	"cyclops-cs-backend/metrics"
)

const firstSeenProperty = "fleet_first_seen_at"
const firstActivationProperty = "fleet_activated_at"

type Config struct {
	Enabled          bool
	Host             string
	ProjectToken     string
	IdentityKey      string
	Environment      string
	ExcludedSubjects []string
	QueueSize        int
	BatchSize        int
	FlushInterval    time.Duration
	RequestTimeout   time.Duration
}

type Client struct {
	config   Config
	http     *http.Client
	queue    chan Event
	done     chan struct{}
	excluded map[string]struct{}
	stopOnce sync.Once
}

func New(config Config) *Client {
	if config.QueueSize <= 0 {
		config.QueueSize = 256
	}
	if config.BatchSize <= 0 {
		config.BatchSize = 20
	}
	if config.FlushInterval <= 0 {
		config.FlushInterval = time.Second
	}
	if config.RequestTimeout <= 0 {
		config.RequestTimeout = 2 * time.Second
	}
	config.Host = strings.TrimRight(config.Host, "/")
	client := &Client{
		config:   config,
		http:     &http.Client{Timeout: config.RequestTimeout},
		queue:    make(chan Event, config.QueueSize),
		done:     make(chan struct{}),
		excluded: make(map[string]struct{}, len(config.ExcludedSubjects)),
	}
	for _, subject := range config.ExcludedSubjects {
		client.excluded[subject] = struct{}{}
	}
	if config.Enabled && config.ProjectToken != "" && config.Host != "" && config.IdentityKey != "" {
		go client.run()
	} else {
		close(client.done)
	}
	return client
}

func (client *Client) Capture(event Event) {
	if !client.config.Enabled || client.config.ProjectToken == "" || client.config.Host == "" || client.config.IdentityKey == "" {
		metrics.RecordProductAnalytics(event.Name, "disabled")
		return
	}
	if _, excluded := client.excluded[event.DistinctID]; excluded {
		metrics.RecordProductAnalytics(event.Name, "excluded")
		return
	}
	if err := ValidateEvent(event); err != nil {
		metrics.RecordProductAnalytics(event.Name, "invalid")
		return
	}
	pseudonym := PseudonymForUserID(event.DistinctID, client.config.IdentityKey)
	event.DistinctID = pseudonym
	if event.Name == EventFleetActivation {
		event.InsertID = "fleet-activation:" + pseudonym
	}
	if event.Name == EventAttributionBound {
		event.InsertID = "fleet-attribution:" + pseudonym
	}
	copied := cloneEvent(event)
	copied.Properties["environment"] = client.config.Environment
	copied.Properties["instrumentation_version"] = Version
	if copied.SetOnce == nil {
		copied.SetOnce = map[string]any{}
	}
	copied.SetOnce[firstSeenProperty] = time.Now().UTC().Format(time.RFC3339)
	select {
	case client.queue <- copied:
		metrics.SetProductAnalyticsQueueDepth(len(client.queue))
		metrics.RecordProductAnalytics(event.Name, "accepted")
	default:
		metrics.RecordProductAnalytics(event.Name, "queue_full")
	}
}

func cloneEvent(event Event) Event {
	properties := make(map[string]any, len(event.Properties))
	for key, value := range event.Properties {
		properties[key] = value
	}
	setOnce := make(map[string]any, len(event.SetOnce))
	for key, value := range event.SetOnce {
		setOnce[key] = value
	}
	event.Properties = properties
	event.SetOnce = setOnce
	return event
}

func (client *Client) run() {
	defer close(client.done)
	ticker := time.NewTicker(client.config.FlushInterval)
	defer ticker.Stop()
	batch := make([]Event, 0, client.config.BatchSize)
	flush := func() {
		if len(batch) == 0 {
			return
		}
		client.deliver(batch)
		batch = batch[:0]
	}
	for {
		select {
		case event, ok := <-client.queue:
			if !ok {
				flush()
				return
			}
			batch = append(batch, event)
			metrics.SetProductAnalyticsQueueDepth(len(client.queue))
			if len(batch) >= client.config.BatchSize {
				flush()
			}
		case <-ticker.C:
			flush()
		}
	}
}

func (client *Client) deliver(events []Event) {
	started := time.Now()
	defer func() { metrics.ObserveProductAnalyticsDelivery(time.Since(started)) }()
	items := make([]map[string]any, 0, len(events))
	for _, event := range events {
		properties := cloneMap(event.Properties)
		if event.InsertID != "" {
			properties["$insert_id"] = event.InsertID
		}
		if len(event.SetOnce) > 0 {
			properties["$set_once"] = cloneMap(event.SetOnce)
		}
		items = append(items, map[string]any{"event": event.Name, "distinct_id": event.DistinctID, "properties": properties})
	}
	body, err := json.Marshal(map[string]any{"api_key": client.config.ProjectToken, "batch": items})
	if err != nil {
		for _, event := range events {
			metrics.RecordProductAnalytics(event.Name, "encode_failed")
		}
		return
	}
	request, err := http.NewRequestWithContext(context.Background(), http.MethodPost, client.config.Host+"/batch/", bytes.NewReader(body))
	if err != nil {
		for _, event := range events {
			metrics.RecordProductAnalytics(event.Name, "delivery_failed")
		}
		return
	}
	request.Header.Set("Content-Type", "application/json")
	response, err := client.http.Do(request)
	if err != nil {
		slog.Warn("product analytics delivery failed", "event_count", len(events))
		for _, event := range events {
			metrics.RecordProductAnalytics(event.Name, "delivery_failed")
		}
		return
	}
	_ = response.Body.Close()
	result := "delivered"
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		result = "delivery_failed"
	}
	for _, event := range events {
		metrics.RecordProductAnalytics(event.Name, result)
	}
}

func cloneMap(input map[string]any) map[string]any {
	output := make(map[string]any, len(input))
	for key, value := range input {
		output[key] = value
	}
	return output
}

func (client *Client) Shutdown(ctx context.Context) error {
	client.stopOnce.Do(func() {
		if client.config.Enabled && client.config.ProjectToken != "" && client.config.Host != "" && client.config.IdentityKey != "" {
			close(client.queue)
		}
	})
	select {
	case <-client.done:
		return nil
	case <-ctx.Done():
		return errors.New("product analytics shutdown timed out")
	}
}
