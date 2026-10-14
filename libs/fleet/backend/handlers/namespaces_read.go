package handlers

import (
	"context"
	"crypto/tls"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptrace"
	"net/url"
	"time"

	"cyclops-cs-backend/identity"
	"cyclops-cs-backend/internal/redactederror"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/open-feature/go-sdk/openfeature"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
)

const namespacePostgresFlag = "/feature-flags/cyclops-cs/ff-list-ns-read-pg"
const namespacePostgresTimeout = 5 * time.Second
const namespaceListSQL = `SELECT object::text FROM k8s_api.current_resources
WHERE api_group = '' AND resource = 'namespaces' AND namespace = '' ORDER BY name`

type namespaceObject struct {
	Metadata struct {
		Name              string            `json:"name"`
		Labels            map[string]string `json:"labels"`
		CreationTimestamp string            `json:"creationTimestamp"`
	} `json:"metadata"`
	Status struct {
		Phase string `json:"phase"`
	} `json:"status"`
}

type namespaceResultWriter struct{ items []namespaceObject }

func (writer *namespaceResultWriter) WriteFieldDescriptions(_ []pgconn.FieldDescription) error {
	return nil
}
func (writer *namespaceResultWriter) WriteRow(values []any) error {
	if len(values) != 1 {
		return errors.New("namespace query returned unexpected columns")
	}
	object, ok := values[0].(string)
	if !ok {
		return errors.New("namespace query returned non-text object")
	}
	var item namespaceObject
	if err := json.Unmarshal([]byte(object), &item); err != nil {
		return redactederror.New("namespace query returned invalid object", err)
	}
	if item.Metadata.Name == "" {
		return errors.New("namespace query returned object without name")
	}
	writer.items = append(writer.items, item)
	return nil
}

func finishNamespaceSpan(span trace.Span, err error) {
	if err != nil {
		span.RecordError(errors.New("namespace operation failed"))
		span.SetAttributes(attribute.String("error.type", fmt.Sprintf("%T", err)))
		span.SetStatus(codes.Error, "namespace operation failed")
	}
	span.End()
}

func namespacePostgresEnabled(ctx context.Context, subject string) bool {
	ctx, span := handlerTracer().Start(ctx, "feature_flag.evaluate", trace.WithAttributes(attribute.String("feature_flag.key", namespacePostgresFlag)))
	callCtx, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()
	enabled, err := openfeature.NewClient("cyclops-cs-namespaces").BooleanValue(callCtx, namespacePostgresFlag, false, openfeature.NewEvaluationContext(subject, nil))
	if err != nil {
		enabled = false
		slog.WarnContext(ctx, "namespace read flag evaluation failed; using Kubernetes", "err", err)
	}
	span.SetAttributes(attribute.Bool("feature_flag.result.value", enabled))
	finishNamespaceSpan(span, err)
	return enabled
}

func namespaceTenant(ctx context.Context, subject string) string {
	ctx, span := handlerTracer().Start(ctx, "identity.resolve_tenant")
	defer span.End()
	return identity.PersonalGroup(ctx, subject)
}

func (h Handlers) readNamespacesPostgres(ctx context.Context, tenant string) (items []namespaceObject, err error) {
	ctx, span := handlerTracer().Start(ctx, "namespaces.read.postgres", trace.WithAttributes(attribute.String("read.backend", "postgres")))
	defer func() { finishNamespaceSpan(span, err) }()
	executor := h.Features.StateQuery()
	if executor == nil {
		return nil, errors.New("namespace PostgreSQL reader unavailable")
	}
	callCtx, cancel := context.WithTimeout(ctx, namespacePostgresTimeout)
	defer cancel()
	writer := &namespaceResultWriter{}
	if err = executor.Execute(callCtx, tenant, namespaceListSQL, writer); err != nil {
		return nil, err
	}
	span.SetAttributes(attribute.Int("namespaces.count", len(writer.items)))
	return writer.items, nil
}

func (h Handlers) readNamespacesK8s(ctx context.Context, tenant, subject string) (items []namespaceObject, status int, err error) {
	ctx, span := handlerTracer().Start(ctx, "namespaces.read.kubernetes", trace.WithAttributes(attribute.String("read.backend", "kubernetes")))
	defer func() { span.SetAttributes(attribute.Int("http.status_code", status)); finishNamespaceSpan(span, err) }()
	path := "/api/v1/namespaces?labelSelector=" + url.QueryEscape("capsule.clastix.io/tenant="+tenant)
	requestCtx, requestSpan := handlerTracer().Start(ctx, "kubernetes.request", trace.WithSpanKind(trace.SpanKindClient), trace.WithAttributes(attribute.String("http.request.method", "GET"), attribute.String("k8s.resource.name", "namespaces")))
	requestCtx = httptrace.WithClientTrace(requestCtx, &httptrace.ClientTrace{
		GetConn: func(_ string) { requestSpan.AddEvent("http.connection.acquire") },
		GotConn: func(info httptrace.GotConnInfo) {
			requestSpan.SetAttributes(attribute.Bool("http.connection.reused", info.Reused))
			requestSpan.AddEvent("http.connection.acquired")
		},
		DNSStart:             func(_ httptrace.DNSStartInfo) { requestSpan.AddEvent("dns.start") },
		DNSDone:              func(_ httptrace.DNSDoneInfo) { requestSpan.AddEvent("dns.done") },
		ConnectStart:         func(_, _ string) { requestSpan.AddEvent("tcp.connect.start") },
		ConnectDone:          func(_, _ string, _ error) { requestSpan.AddEvent("tcp.connect.done") },
		TLSHandshakeStart:    func() { requestSpan.AddEvent("tls.handshake.start") },
		TLSHandshakeDone:     func(_ tls.ConnectionState, _ error) { requestSpan.AddEvent("tls.handshake.done") },
		GotFirstResponseByte: func() { requestSpan.AddEvent("http.response.first_byte") },
	})
	resp, requestErr := h.k8sImpersonate(requestCtx, "GET", path, nil, subject)
	if requestErr != nil {
		finishNamespaceSpan(requestSpan, requestErr)
		slog.WarnContext(ctx, "namespace list: k8s request failed", "err", requestErr)
		return nil, http.StatusBadGateway, redactederror.New("kubectl-proxy unavailable", requestErr)
	}
	defer resp.Body.Close()
	requestSpan.SetAttributes(attribute.Int("http.response.status_code", resp.StatusCode))
	if resp.StatusCode != http.StatusOK {
		body, readErr := io.ReadAll(resp.Body)
		statusErr := fmt.Errorf("k8s: %s", body)
		if readErr != nil {
			statusErr = redactederror.New("k8s response read failed", readErr)
		}
		finishNamespaceSpan(requestSpan, statusErr)
		return nil, resp.StatusCode, statusErr
	}
	finishNamespaceSpan(requestSpan, nil)
	_, decodeSpan := handlerTracer().Start(ctx, "namespaces.decode")
	var list struct {
		Items []namespaceObject `json:"items"`
	}
	decodeErr := json.NewDecoder(resp.Body).Decode(&list)
	finishNamespaceSpan(decodeSpan, decodeErr)
	if decodeErr != nil {
		return nil, http.StatusBadGateway, redactederror.New("bad response from k8s", decodeErr)
	}
	span.SetAttributes(attribute.Int("namespaces.count", len(list.Items)))
	return list.Items, http.StatusOK, nil
}
