package handlers

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"log"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"cyclops-cs-backend/config"
	"cyclops-cs-backend/middlewares"
	"cyclops-cs-backend/signedurls"

	"github.com/google/uuid"
	"go.opentelemetry.io/otel"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/sdk/trace/tracetest"
)

type fakeSignedSvcService struct {
	record signedurls.Record
	errors map[string]error
}

func (service fakeSignedSvcService) Create(context.Context, signedurls.CreateInput) (signedurls.Record, error) {
	return signedurls.Record{}, errors.New("unexpected Create call")
}

func (service fakeSignedSvcService) List(context.Context, string, string) ([]signedurls.Record, error) {
	return nil, errors.New("unexpected List call")
}

func (service fakeSignedSvcService) Revoke(context.Context, string, uuid.UUID) (signedurls.Record, error) {
	return signedurls.Record{}, errors.New("unexpected Revoke call")
}

func (service fakeSignedSvcService) Resolve(_ context.Context, token string, _ time.Time) (signedurls.Record, error) {
	if err := service.errors[token]; err != nil {
		return signedurls.Record{}, err
	}
	return service.record, nil
}

type errorReadCloser struct {
	err error
}

func (body errorReadCloser) Read([]byte) (int, error) {
	return 0, body.err
}

func (errorReadCloser) Close() error {
	return nil
}

type roundTripperFunc func(*http.Request) (*http.Response, error)

func (function roundTripperFunc) RoundTrip(request *http.Request) (*http.Response, error) {
	return function(request)
}

func TestHandleSignedSvc(t *testing.T) {
	const validToken = "valid-capability-token"
	record := signedurls.Record{
		ID:          uuid.MustParse("c8bc94ca-9514-47f2-97a6-8a84e8fc3a60"),
		Namespace:   "ns-a",
		ServiceName: "sandbox-a-mcp",
	}
	service := fakeSignedSvcService{
		record: record,
		errors: map[string]error{
			"malformed":               signedurls.ErrInvalidCapability,
			strings.Repeat("a", 4097): signedurls.ErrInvalidCapability,
			"tampered":                signedurls.ErrInvalidCapability,
			"expired":                 signedurls.ErrInvalidCapability,
			"unknown":                 signedurls.ErrInvalidCapability,
			"row-mismatched":          signedurls.ErrInvalidCapability,
			"missing-service":         signedurls.ErrInvalidCapability,
			"revoked":                 signedurls.ErrInvalidCapability,
		},
	}

	var (
		upstreamCalls int
		upstreamURL   string
		upstreamHdr   http.Header
	)
	previousTransport := http.DefaultTransport
	http.DefaultTransport = roundTripperFunc(func(request *http.Request) (*http.Response, error) {
		upstreamCalls++
		upstreamURL = request.URL.String()
		upstreamHdr = request.Header.Clone()
		return &http.Response{
			StatusCode: http.StatusOK,
			Header:     http.Header{"Content-Type": {"application/octet-stream"}},
			Body:       io.NopCloser(bytes.NewReader([]byte{0x00, 0xff, 0x42})),
			Request:    request,
		}, nil
	})
	t.Cleanup(func() { http.DefaultTransport = previousTransport })

	var logs bytes.Buffer
	previousLogger := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(&logs, nil)))
	t.Cleanup(func() { slog.SetDefault(previousLogger) })

	spanRecorder := tracetest.NewSpanRecorder()
	tracerProvider := sdktrace.NewTracerProvider(sdktrace.WithSpanProcessor(spanRecorder))
	previousTracerProvider := otel.GetTracerProvider()
	otel.SetTracerProvider(tracerProvider)
	t.Cleanup(func() {
		otel.SetTracerProvider(previousTracerProvider)
		_ = tracerProvider.Shutdown(context.Background())
	})

	handler := Handlers{
		GatewayCfg:          config.GatewayConfiguration{Scheme: "http", Port: "80", ClusterDomain: "svc.cluster.local"},
		signedServiceURLs:   service,
		signedServiceExists: func(context.Context, string, string, string) (bool, error) { return true, nil },
	}
	invalidBody := ""
	for _, testCase := range []struct {
		name         string
		token        string
		path         string
		wantUpstream string
		wantStatus   int
	}{
		{
			name:         "valid capability proxies binary response",
			token:        validToken,
			path:         "/api/signed-svc/" + validToken + "/tools?cursor=next",
			wantUpstream: "http://sandbox-a-mcp.ns-a.svc.cluster.local:80/tools?cursor=next",
			wantStatus:   http.StatusOK,
		},
		{name: "malformed capability is indistinguishable", token: "malformed", wantStatus: http.StatusNotFound},
		{name: "oversized capability is indistinguishable", token: strings.Repeat("a", 4097), wantStatus: http.StatusNotFound},
		{name: "tampered capability is indistinguishable", token: "tampered", wantStatus: http.StatusNotFound},
		{name: "expired capability is indistinguishable", token: "expired", wantStatus: http.StatusNotFound},
		{name: "unknown capability is indistinguishable", token: "unknown", wantStatus: http.StatusNotFound},
		{name: "row-mismatched capability is indistinguishable", token: "row-mismatched", wantStatus: http.StatusNotFound},
		{name: "revoked capability is indistinguishable", token: "revoked", wantStatus: http.StatusNotFound},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			path := testCase.path
			if path == "" {
				path = "/api/signed-svc/" + testCase.token
			}
			request := httptest.NewRequest(http.MethodGet, path, nil)
			request.Header.Set("Authorization", "Bearer must-not-reach-sandbox")
			request.Header.Set("Proxy-Authorization", "Basic must-not-reach-sandbox")
			request.Header.Set("Connection", "X-Private-Hop")
			request.Header.Set("X-Private-Hop", "must-not-reach-sandbox")
			request.Header.Set("Cookie", "app_session=keep; _oauth2_proxy_svc=must-not-reach-sandbox")
			request.Header.Set("X-Auth-Request-User", "must-not-reach-sandbox")
			request.Header.Set("X-Auth-Request-Email", "must-not-reach-sandbox")
			request.Header.Set("X-Auth-Request-Access-Token", "must-not-reach-sandbox")
			request.Header.Set("X-Auth-Request-Groups", "must-not-reach-sandbox")
			request.SetPathValue("token", testCase.token)
			if strings.Contains(testCase.path, "/tools") {
				request.SetPathValue("path", "tools")
			}
			response := httptest.NewRecorder()

			handler.HandleSignedSvc(response, request)

			if response.Code != testCase.wantStatus {
				t.Fatalf("status = %d, want %d; body = %q", response.Code, testCase.wantStatus, response.Body.String())
			}
			if testCase.wantUpstream != "" {
				if upstreamURL != testCase.wantUpstream {
					t.Fatalf("upstream URL = %q, want %q", upstreamURL, testCase.wantUpstream)
				}
				if got := response.Body.Bytes(); !bytes.Equal(got, []byte{0x00, 0xff, 0x42}) {
					t.Fatalf("body = %x, want 00ff42", got)
				}
				for _, header := range []string{"Authorization", "Proxy-Authorization", "Connection", "X-Private-Hop", "X-Auth-Request-User", "X-Auth-Request-Email", "X-Auth-Request-Access-Token", "X-Auth-Request-Groups"} {
					if got := upstreamHdr.Get(header); got != "" {
						t.Fatalf("upstream %s = %q, want empty", header, got)
					}
				}
				if got, want := upstreamHdr.Get("Cookie"), "app_session=keep"; got != want {
					t.Fatalf("upstream Cookie = %q, want %q", got, want)
				}
				return
			}
			if invalidBody == "" {
				invalidBody = response.Body.String()
			} else if response.Body.String() != invalidBody {
				t.Fatalf("body = %q, want indistinguishable %q", response.Body.String(), invalidBody)
			}
		})
	}

	if upstreamCalls != 1 {
		t.Fatalf("upstream calls = %d, want 1", upstreamCalls)
	}
	if !strings.Contains(logs.String(), record.ID.String()) {
		t.Fatalf("logs do not contain resolved URL record ID %q: %s", record.ID, logs.String())
	}
	for _, token := range []string{validToken, "malformed", "tampered", "expired", "unknown", "row-mismatched", "revoked"} {
		if strings.Contains(logs.String(), token) {
			t.Fatalf("logs contain capability token %q: %s", token, logs.String())
		}
		for _, span := range spanRecorder.Ended() {
			if strings.Contains(fmt.Sprint(span.Attributes()), token) {
				t.Fatalf("span %q attributes contain capability token %q: %v", span.Name(), token, span.Attributes())
			}
		}
	}
}

func TestHandleSignedSvcInvalidatesCapabilityWhenKubernetesServiceIsGone(t *testing.T) {
	fakeK8s := newFakeK8s(http.StatusNotFound, `{"kind":"Status"}`)
	t.Cleanup(fakeK8s.server.Close)
	overrideK8sClient(fakeK8s.server.Client(), fakeK8s.server.URL, "fake-sa-token")
	previousTransport := http.DefaultTransport
	http.DefaultTransport = roundTripperFunc(func(*http.Request) (*http.Response, error) {
		t.Fatal("missing Kubernetes Service reached upstream proxy")
		return nil, nil
	})
	t.Cleanup(func() { http.DefaultTransport = previousTransport })
	const token = "valid-capability-token"
	handler := Handlers{GatewayCfg: config.GatewayConfiguration{Scheme: "http", Port: "80", ClusterDomain: "svc.cluster.local"}, signedServiceURLs: fakeSignedSvcService{record: signedurls.Record{ID: uuid.MustParse("c8bc94ca-9514-47f2-97a6-8a84e8fc3a60"), Namespace: "ns-a", ServiceName: "sandbox-a-mcp"}}, checkSignedServiceExists: true}
	request := httptest.NewRequest(http.MethodGet, "/api/signed-svc/"+token, nil)
	request.SetPathValue("token", token)
	response := httptest.NewRecorder()
	handler.HandleSignedSvc(response, request)
	if response.Code != http.StatusNotFound {
		t.Fatalf("status = %d, want 404; body = %q", response.Code, response.Body.String())
	}
	if len(fakeK8s.requests) != 1 || fakeK8s.requests[0].method != http.MethodGet || fakeK8s.requests[0].path != "/api/v1/namespaces/ns-a/services/sandbox-a-mcp" {
		t.Fatalf("Kubernetes requests = %#v, want GET service existence check", fakeK8s.requests)
	}
}

func TestHandleSignedSvcRewritesRedirects(t *testing.T) {
	const token = "redirect-capability-token"
	previousTransport := http.DefaultTransport
	http.DefaultTransport = roundTripperFunc(func(request *http.Request) (*http.Response, error) {
		return &http.Response{
			StatusCode: http.StatusFound,
			Header:     http.Header{"Location": {"/login"}},
			Body:       io.NopCloser(strings.NewReader("")),
			Request:    request,
		}, nil
	})
	t.Cleanup(func() { http.DefaultTransport = previousTransport })

	handler := Handlers{
		GatewayCfg: config.GatewayConfiguration{Scheme: "http", Port: "80", ClusterDomain: "svc.cluster.local"},
		signedServiceURLs: fakeSignedSvcService{record: signedurls.Record{
			ID: uuid.MustParse("c8bc94ca-9514-47f2-97a6-8a84e8fc3a60"), Namespace: "ns-a", ServiceName: "sandbox-a-mcp",
		}},
	}
	request := httptest.NewRequest(http.MethodGet, "/api/signed-svc/"+token+"/tools", nil)
	request.SetPathValue("token", token)
	request.SetPathValue("path", "tools")
	response := httptest.NewRecorder()

	handler.HandleSignedSvc(response, request)

	if response.Code != http.StatusFound {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusFound)
	}
	if got, want := response.Header().Get("Location"), "/api/signed-svc/"+token+"/login"; got != want {
		t.Fatalf("Location = %q, want %q", got, want)
	}
}

func TestRewriteLocationUsesCurrentProxiedPath(t *testing.T) {
	const basePath = "/api/signed-svc/signed-capability-token"
	for _, testCase := range []struct {
		name         string
		upstreamPath string
		location     string
		want         string
	}{
		{name: "path-relative", upstreamPath: "/tools/current", location: "next", want: basePath + "/tools/next"},
		{name: "query-only", upstreamPath: "/tools/current", location: "?cursor=next", want: basePath + "/tools/current?cursor=next"},
		{name: "dot-segment remains under signed base", upstreamPath: "/tools/current", location: "../../../escape", want: basePath + "/escape"},
		{name: "encoded relative dot-segment remains under signed base", upstreamPath: "/tools/current", location: "%2e%2e/escape", want: basePath + "/escape"},
		{name: "encoded absolute dot-segment remains under signed base", upstreamPath: "/tools/current", location: "/%2e%2e/escape", want: basePath + "/escape"},
		{name: "absolute upstream path", upstreamPath: "/tools/current", location: "/login", want: basePath + "/login"},
		{name: "external URL", upstreamPath: "/tools/current", location: "https://example.com/login", want: "https://example.com/login"},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			if got := rewriteLocation(testCase.location, basePath, testCase.upstreamPath); got != testCase.want {
				t.Fatalf("rewriteLocation(%q, %q, %q) = %q, want %q", testCase.location, basePath, testCase.upstreamPath, got, testCase.want)
			}
		})
	}
}

func TestHandleSignedSvcPreservesEscapedPath(t *testing.T) {
	const token = "encoded-path-capability-token"
	var escapedPath string
	previousTransport := http.DefaultTransport
	http.DefaultTransport = roundTripperFunc(func(request *http.Request) (*http.Response, error) {
		escapedPath = request.URL.EscapedPath()
		return &http.Response{StatusCode: http.StatusNoContent, Body: io.NopCloser(strings.NewReader("")), Request: request}, nil
	})
	t.Cleanup(func() { http.DefaultTransport = previousTransport })

	handler := Handlers{
		GatewayCfg: config.GatewayConfiguration{Scheme: "http", Port: "80", ClusterDomain: "svc.cluster.local"},
		signedServiceURLs: fakeSignedSvcService{record: signedurls.Record{
			ID: uuid.MustParse("c8bc94ca-9514-47f2-97a6-8a84e8fc3a60"), Namespace: "ns-a", ServiceName: "sandbox-a-mcp",
		}},
	}
	request := httptest.NewRequest(http.MethodGet, "/api/signed-svc/"+token+"/files/a%2Fb", nil)
	request.SetPathValue("token", token)
	request.SetPathValue("path", "files/a/b")
	response := httptest.NewRecorder()

	handler.HandleSignedSvc(response, request)

	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusNoContent)
	}
	if escapedPath != "/files/a%2Fb" {
		t.Fatalf("upstream escaped path = %q, want %q", escapedPath, "/files/a%2Fb")
	}
}

func TestHandleSignedSvcUsesMatchedTelemetryRoute(t *testing.T) {
	const token = "telemetry-route-capability-token"
	previousTransport := http.DefaultTransport
	http.DefaultTransport = roundTripperFunc(func(request *http.Request) (*http.Response, error) {
		return &http.Response{StatusCode: http.StatusNoContent, Body: io.NopCloser(strings.NewReader("")), Request: request}, nil
	})
	t.Cleanup(func() { http.DefaultTransport = previousTransport })

	handler := Handlers{
		GatewayCfg: config.GatewayConfiguration{Scheme: "http", Port: "80", ClusterDomain: "svc.cluster.local"},
		signedServiceURLs: fakeSignedSvcService{record: signedurls.Record{
			ID: uuid.MustParse("c8bc94ca-9514-47f2-97a6-8a84e8fc3a60"), Namespace: "ns-a", ServiceName: "sandbox-a-mcp",
		}},
	}
	for _, testCase := range []struct {
		name  string
		path  string
		value string
		route string
		want  string
	}{
		{name: "root", path: "/api/signed-svc/" + token, want: "/api/signed-svc/{token}"},
		{name: "wildcard", path: "/api/signed-svc/" + token + "/tools", value: "tools", want: "/api/signed-svc/{token}/{path...}"},
		{name: "wildcard empty", path: "/api/signed-svc/" + token + "/", route: "/api/signed-svc/{token}/{path...}", want: "/api/signed-svc/{token}/{path...}"},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			spanRecorder := tracetest.NewSpanRecorder()
			tracerProvider := sdktrace.NewTracerProvider(sdktrace.WithSpanProcessor(spanRecorder))
			previousTracerProvider := otel.GetTracerProvider()
			otel.SetTracerProvider(tracerProvider)
			t.Cleanup(func() {
				otel.SetTracerProvider(previousTracerProvider)
				_ = tracerProvider.Shutdown(context.Background())
			})

			request := httptest.NewRequest(http.MethodGet, testCase.path, nil)
			if testCase.route != "" {
				request = request.WithContext(context.WithValue(request.Context(), middlewares.ContextKey("route"), testCase.route))
			}
			request.SetPathValue("token", token)
			if testCase.value != "" {
				request.SetPathValue("path", testCase.value)
			}
			handler.HandleSignedSvc(httptest.NewRecorder(), request)

			spans := spanRecorder.Ended()
			if len(spans) != 1 {
				t.Fatalf("ended spans = %d, want 1", len(spans))
			}
			for _, attribute := range spans[0].Attributes() {
				if attribute.Key == "http.route" && attribute.Value.AsString() == testCase.want {
					return
				}
			}
			t.Fatalf("http.route attribute = %v, want %q", spans[0].Attributes(), testCase.want)
		})
	}
}

func TestHandleSignedSvcDoesNotExposeCapabilitySuffixOnProxyFailure(t *testing.T) {
	const token = "capability-token-repeated-in-suffix"
	record := signedurls.Record{
		ID:          uuid.MustParse("c8bc94ca-9514-47f2-97a6-8a84e8fc3a60"),
		Namespace:   "ns-a",
		ServiceName: "sandbox-a-mcp",
	}
	for _, testCase := range []struct {
		name string
		err  error
		want int
	}{
		{name: "proxy error", err: errors.New("upstream failed for " + token), want: http.StatusBadGateway},
		{name: "client cancellation", err: context.Canceled, want: statusClientClosedRequest},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			previousTransport := http.DefaultTransport
			http.DefaultTransport = roundTripperFunc(func(*http.Request) (*http.Response, error) {
				return nil, testCase.err
			})
			t.Cleanup(func() { http.DefaultTransport = previousTransport })

			var logs bytes.Buffer
			previousLogger := slog.Default()
			slog.SetDefault(slog.New(slog.NewTextHandler(&logs, nil)))
			t.Cleanup(func() { slog.SetDefault(previousLogger) })

			spanRecorder := tracetest.NewSpanRecorder()
			tracerProvider := sdktrace.NewTracerProvider(sdktrace.WithSpanProcessor(spanRecorder))
			previousTracerProvider := otel.GetTracerProvider()
			otel.SetTracerProvider(tracerProvider)
			t.Cleanup(func() {
				otel.SetTracerProvider(previousTracerProvider)
				_ = tracerProvider.Shutdown(context.Background())
			})

			handler := Handlers{
				GatewayCfg:        config.GatewayConfiguration{Scheme: "http", Port: "80", ClusterDomain: "svc.cluster.local"},
				signedServiceURLs: fakeSignedSvcService{record: record},
			}
			request := httptest.NewRequest(http.MethodGet, "/api/signed-svc/"+token+"/"+token, nil)
			request.SetPathValue("token", token)
			request.SetPathValue("path", token)
			response := httptest.NewRecorder()

			handler.HandleSignedSvc(response, request)

			if response.Code != testCase.want {
				t.Fatalf("status = %d, want %d", response.Code, testCase.want)
			}
			if strings.Contains(logs.String(), token) {
				t.Fatalf("logs leaked capability suffix: %s", logs.String())
			}
			if !strings.Contains(logs.String(), record.ID.String()) {
				t.Fatalf("logs do not contain signed URL record ID %q: %s", record.ID, logs.String())
			}
			for _, span := range spanRecorder.Ended() {
				if strings.Contains(fmt.Sprint(span.Attributes()), token) || strings.Contains(fmt.Sprint(span.Events()), token) {
					t.Fatalf("span leaked capability suffix: attributes=%v events=%v", span.Attributes(), span.Events())
				}
			}
		})
	}
}

func TestHandleSignedSvcPreservesEscapedPathAfterEscapedToken(t *testing.T) {
	const token = "capability~token"
	var escapedPath string
	previousTransport := http.DefaultTransport
	http.DefaultTransport = roundTripperFunc(func(request *http.Request) (*http.Response, error) {
		escapedPath = request.URL.EscapedPath()
		return &http.Response{StatusCode: http.StatusNoContent, Body: io.NopCloser(strings.NewReader("")), Request: request}, nil
	})
	t.Cleanup(func() { http.DefaultTransport = previousTransport })

	handler := Handlers{
		GatewayCfg: config.GatewayConfiguration{Scheme: "http", Port: "80", ClusterDomain: "svc.cluster.local"},
		signedServiceURLs: fakeSignedSvcService{record: signedurls.Record{
			ID: uuid.MustParse("c8bc94ca-9514-47f2-97a6-8a84e8fc3a60"), Namespace: "ns-a", ServiceName: "sandbox-a-mcp",
		}},
	}
	request := httptest.NewRequest(http.MethodGet, "/api/signed-svc/capability%7Etoken/files/a%2Fb", nil)
	request.SetPathValue("token", token)
	request.SetPathValue("path", "files/a/b")
	response := httptest.NewRecorder()

	handler.HandleSignedSvc(response, request)

	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusNoContent)
	}
	if escapedPath != "/files/a%2Fb" {
		t.Fatalf("upstream escaped path = %q, want %q", escapedPath, "/files/a%2Fb")
	}
}

func TestHandleSignedSvcDiscardsBodyCopyErrorLog(t *testing.T) {
	const token = "capability-token-repeated-in-malformed-body"
	previousTransport := http.DefaultTransport
	http.DefaultTransport = roundTripperFunc(func(request *http.Request) (*http.Response, error) {
		return &http.Response{
			StatusCode: http.StatusOK,
			Header:     make(http.Header),
			Body:       errorReadCloser{err: errors.New("malformed chunked body contains " + token)},
			Request:    request,
		}, nil
	})
	t.Cleanup(func() { http.DefaultTransport = previousTransport })

	var logs bytes.Buffer
	previousOutput := log.Writer()
	previousFlags := log.Flags()
	previousPrefix := log.Prefix()
	log.SetOutput(&logs)
	log.SetFlags(0)
	log.SetPrefix("")
	t.Cleanup(func() {
		log.SetOutput(previousOutput)
		log.SetFlags(previousFlags)
		log.SetPrefix(previousPrefix)
	})

	handler := Handlers{
		GatewayCfg: config.GatewayConfiguration{Scheme: "http", Port: "80", ClusterDomain: "svc.cluster.local"},
		signedServiceURLs: fakeSignedSvcService{record: signedurls.Record{
			ID: uuid.MustParse("c8bc94ca-9514-47f2-97a6-8a84e8fc3a60"), Namespace: "ns-a", ServiceName: "sandbox-a-mcp",
		}},
	}
	request := httptest.NewRequest(http.MethodGet, "/api/signed-svc/"+token+"/"+token, nil)
	request.SetPathValue("token", token)
	request.SetPathValue("path", token)
	response := httptest.NewRecorder()

	handler.HandleSignedSvc(response, request)

	if response.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusOK)
	}
	if strings.Contains(logs.String(), token) {
		t.Fatalf("copy error log leaked capability token: %s", logs.String())
	}
}

func TestHandleSignedSvcMapsResolverErrorsWithoutLeakingDetails(t *testing.T) {
	for _, testCase := range []struct {
		name       string
		err        error
		wantStatus int
	}{
		{name: "invalid capability", err: signedurls.ErrInvalidCapability, wantStatus: http.StatusNotFound},
		{name: "dependency unavailable", err: signedurls.ErrUnavailable, wantStatus: http.StatusServiceUnavailable},
		{name: "unexpected internal failure", err: errors.New("secret database detail"), wantStatus: http.StatusInternalServerError},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			token := "sensitive-capability-token"
			handler := Handlers{signedServiceURLs: fakeSignedSvcService{errors: map[string]error{token: testCase.err}}}
			request := httptest.NewRequest(http.MethodGet, "/api/signed-svc/"+token, nil)
			request.SetPathValue("token", token)
			response := httptest.NewRecorder()

			handler.HandleSignedSvc(response, request)

			if response.Code != testCase.wantStatus {
				t.Fatalf("status = %d, want %d; body = %q", response.Code, testCase.wantStatus, response.Body.String())
			}
			if strings.Contains(response.Body.String(), token) || strings.Contains(response.Body.String(), "secret database detail") {
				t.Fatalf("response leaked sensitive detail: %q", response.Body.String())
			}
		})
	}
}
