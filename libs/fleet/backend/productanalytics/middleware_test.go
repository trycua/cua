package productanalytics

import (
	"bufio"
	"context"
	"net"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"go.opentelemetry.io/otel/trace"
)

type captureSink struct{ events []Event }

func (sink *captureSink) Capture(event Event) { sink.events = append(sink.events, event) }

type eventChannel chan Event

func (sink eventChannel) Capture(event Event) { sink <- event }

func observedRequest(method, route, path string, user *auth.User) *http.Request {
	request := httptest.NewRequest(method, "http://example.test/api", nil)
	request.SetPathValue("path", path)
	ctx := context.WithValue(request.Context(), auth.UserKey, user)
	traceID, _ := trace.TraceIDFromHex("11111111111111111111111111111111")
	spanID, _ := trace.SpanIDFromHex("2222222222222222")
	ctx = trace.ContextWithSpanContext(ctx, trace.NewSpanContext(trace.SpanContextConfig{TraceID: traceID, SpanID: spanID}))
	return request.WithContext(ctx)
}

func TestRouteObserverEmitsActivationOnlyForExternalIdentity(t *testing.T) {
	users := []struct {
		name string
		user *auth.User
		want int
	}{
		{name: "external", user: &auth.User{ID: "external-1", Email: "person@example.test", EmailVerified: true, AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, want: 3},
		{name: "internal", user: &auth.User{ID: "internal-1", Email: "person@trycua.com", EmailVerified: true, AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, want: 1},
		{name: "unknown", user: &auth.User{ID: "unknown-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, want: 1},
	}
	for _, test := range users {
		t.Run(test.name, func(t *testing.T) {
			sink := make(eventChannel, 3)
			handler := RouteObserver("/api/svc/{namespace}/{service}/{path...}", sink, "cyclops-cs-spa", func(context.Context, *http.Request, int) bool { return true })(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusOK) }))
			handler.ServeHTTP(httptest.NewRecorder(), observedRequest(http.MethodPost, "/api/svc/{namespace}/{service}/{path...}", "tools", test.user))
			events := make([]Event, 0, test.want)
			deadline := time.After(time.Second)
			for len(events) < test.want {
				select {
				case event := <-sink:
					events = append(events, event)
				case <-deadline:
					t.Fatalf("events = %d, want %d: %#v", len(events), test.want, events)
				}
			}
			if test.want == 3 {
				var workload, activation *Event
				for i := range events {
					switch events[i].Name {
					case EventQualifyingWorkload:
						workload = &events[i]
					case EventFleetActivation:
						activation = &events[i]
					}
				}
				if workload == nil {
					t.Fatalf("qualifying workload event missing: %#v", events)
				}
				if activation == nil || activation.InsertID != "fleet-activation:"+test.user.ID || activation.SetOnce[firstActivationProperty] == nil {
					t.Fatalf("activation event = %#v", activation)
				}
			}
		})
	}
}

func TestRouteObserverDoesNotBlockOnQualification(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	sink := &captureSink{}
	handler := RouteObserver("/api/svc/{namespace}/{service}/{path...}", sink, "cyclops-cs-spa", func(context.Context, *http.Request, int) bool {
		close(started)
		<-release
		return true
	})(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusOK) }))
	response := httptest.NewRecorder()
	done := make(chan struct{})
	go func() {
		handler.ServeHTTP(response, observedRequest(http.MethodPost, "/api/svc/{namespace}/{service}/{path...}", "tools", &auth.User{ID: "external-1", Email: "person@example.test", EmailVerified: true, AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}))
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(100 * time.Millisecond):
		t.Fatal("request blocked on qualification")
	}
	close(release)
	<-started
}

func TestRouteObserverRecoversQualifierPanic(t *testing.T) {
	started := make(chan struct{})
	handler := RouteObserver("/api/svc/{namespace}/{service}/{path...}", &captureSink{}, "cyclops-cs-spa", func(context.Context, *http.Request, int) bool {
		close(started)
		panic("qualifier failed")
	})(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusOK) }))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, observedRequest(http.MethodPost, "/api/svc/{namespace}/{service}/{path...}", "tools", &auth.User{ID: "external-1", Email: "person@example.test", EmailVerified: true, AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}))
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("qualifier did not run")
	}
	if response.Code != http.StatusOK {
		t.Fatalf("response status = %d", response.Code)
	}
}

func TestRouteObserverCapturesActivationOutcomes(t *testing.T) {
	tests := []struct {
		name, route, method, path                     string
		status                                        int
		user                                          *auth.User
		wantEvent, wantOutcome, wantSource, wantClass string
	}{
		{name: "pool success", route: "/api/k8s/{path...}", method: http.MethodPost, path: "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", status: 201, user: &auth.User{ID: "u-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, wantEvent: EventPoolCreate, wantOutcome: OutcomeSuccess, wantSource: SourceSPA},
		{name: "warm pool failure", route: "/api/k8s/{path...}", method: http.MethodPost, path: "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxwarmpools", status: 422, user: &auth.User{ID: "u-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, wantEvent: EventPoolCreate, wantOutcome: OutcomeFailure, wantSource: SourceSPA, wantClass: "validation"},
		{name: "pool template success", route: "/api/k8s/{path...}", method: http.MethodPost, path: "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates", status: 201, user: &auth.User{ID: "u-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, wantEvent: EventPoolCreate, wantOutcome: OutcomeSuccess, wantSource: SourceSPA},
		{name: "pool template failure", route: "/api/k8s/{path...}", method: http.MethodPost, path: "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates", status: 500, user: &auth.User{ID: "u-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, wantEvent: EventPoolCreate, wantOutcome: OutcomeFailure, wantSource: SourceSPA, wantClass: "upstream_5xx"},
		{name: "claim authorization", route: "/api/k8s/{path...}", method: http.MethodPost, path: "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims", status: 403, user: &auth.User{ID: "u-1", AZP: "ukey-one", PrincipalType: auth.PrincipalTypeUserKey}, wantEvent: EventClaimCreate, wantOutcome: OutcomeFailure, wantSource: SourceUserKey, wantClass: "authorization"},
		{name: "proxy redirect", route: "/api/svc/{namespace}/{service}/{path...}", method: http.MethodGet, status: 302, user: &auth.User{ID: "u-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, wantEvent: EventHTTPProxyRequest, wantOutcome: OutcomeSuccess, wantSource: SourceSPA},
		{name: "proxy upstream", route: "/api/svc/{namespace}/{service}", method: http.MethodPost, status: 502, user: &auth.User{ID: "u-1", AZP: "ukey-one", PrincipalType: auth.PrincipalTypeUserKey}, wantEvent: EventHTTPProxyRequest, wantOutcome: OutcomeFailure, wantSource: SourceUserKey, wantClass: "upstream_5xx"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			sink := &captureSink{}
			handler := RouteObserver(test.route, sink, "cyclops-cs-spa")(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(test.status) }))
			handler.ServeHTTP(httptest.NewRecorder(), observedRequest(test.method, test.route, test.path, test.user))
			if len(sink.events) != 1 {
				t.Fatalf("events = %#v", sink.events)
			}
			event := sink.events[0]
			if event.Name != test.wantEvent || event.InsertID != "11111111111111111111111111111111" {
				t.Fatalf("event = %#v", event)
			}
			if event.Properties["outcome"] != test.wantOutcome || event.Properties["source"] != test.wantSource || event.Properties["error_class"] != test.wantClass {
				t.Fatalf("properties = %#v", event.Properties)
			}
		})
	}
}

func TestRouteObserverIgnoresNonActivationRequests(t *testing.T) {
	tests := []struct {
		method, route, path string
		user                *auth.User
	}{
		{method: http.MethodGet, route: "/api/k8s/{path...}", path: "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", user: &auth.User{ID: "u-1", AZP: "cyclops-cs-spa"}},
		{method: http.MethodPost, route: "/api/k8s/{path...}", path: "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools/name", user: &auth.User{ID: "u-1", AZP: "cyclops-cs-spa"}},
		{method: http.MethodPost, route: "/api/k8s/{path...}", path: "api/v1/namespaces/ns-a/pods", user: &auth.User{ID: "u-1", AZP: "cyclops-cs-spa"}},
		{method: http.MethodGet, route: "/api/gateway/{name}/{path...}", user: &auth.User{ID: "u-1", AZP: "cyclops-cs-spa"}},
		{method: http.MethodGet, route: "/api/svc/{namespace}/{service}", user: &auth.User{ID: "owner", AZP: "github-oidc", PrincipalType: auth.PrincipalTypeGitHubOIDC}},
	}
	for _, test := range tests {
		sink := &captureSink{}
		handler := RouteObserver(test.route, sink, "cyclops-cs-spa")(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(200) }))
		handler.ServeHTTP(httptest.NewRecorder(), observedRequest(test.method, test.route, test.path, test.user))
		if len(sink.events) != 0 {
			t.Fatalf("request %#v captured %#v", test, sink.events)
		}
	}
}

func TestRouteObserverSuppressesSuccessfulWarmPoolCreation(t *testing.T) {
	sink := &captureSink{}
	handlerCalled := false
	handler := RouteObserver("/api/k8s/{path...}", sink, "cyclops-cs-spa")(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		handlerCalled = true
		w.WriteHeader(http.StatusCreated)
	}))
	request := observedRequest(
		http.MethodPost,
		"/api/k8s/{path...}",
		"apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxwarmpools",
		&auth.User{ID: "u-1", AZP: "cyclops-cs-spa"},
	)

	handler.ServeHTTP(httptest.NewRecorder(), request)

	if !handlerCalled {
		t.Fatal("wrapped handler was not called")
	}
	if len(sink.events) != 0 {
		t.Fatalf("captured events = %#v", sink.events)
	}
}

type interfaceWriter struct {
	header http.Header
	status int
}

func (w *interfaceWriter) Header() http.Header {
	if w.header == nil {
		w.header = http.Header{}
	}
	return w.header
}
func (w *interfaceWriter) Write(body []byte) (int, error) {
	if w.status == 0 {
		w.status = 200
	}
	return len(body), nil
}
func (w *interfaceWriter) WriteHeader(status int)                       { w.status = status }
func (w *interfaceWriter) Flush()                                       {}
func (w *interfaceWriter) Hijack() (net.Conn, *bufio.ReadWriter, error) { return nil, nil, nil }

func TestAnalyticsResponseWriterPreservesProxyInterfaces(t *testing.T) {
	wrapped := newAnalyticsResponseWriter(&interfaceWriter{})
	if _, ok := any(wrapped).(http.Flusher); !ok {
		t.Fatal("missing http.Flusher")
	}
	if _, ok := any(wrapped).(http.Hijacker); !ok {
		t.Fatal("missing http.Hijacker")
	}
}
