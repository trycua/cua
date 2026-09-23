package productanalytics

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"go.opentelemetry.io/otel/trace"
)

type captureSink struct{ events []Event }

func (sink *captureSink) Capture(event Event) { sink.events = append(sink.events, event) }

type captureFunc func(Event)

func (capture captureFunc) Capture(event Event) { capture(event) }

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

func TestLoginObserverCapturesCLIAuthenticationOncePerSession(t *testing.T) {
	sink := &captureSink{}
	observer := LoginObserver(sink, "cyclops-cs-spa")
	handler := observer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusForbidden)
	}))
	user := &auth.User{ID: "external-1", Email: "person@example.test", EmailVerified: true, AZP: "cua-cli", PrincipalType: auth.PrincipalTypeUser, Claims: map[string]string{"sid": "session-1"}}
	handler.ServeHTTP(httptest.NewRecorder(), observedRequest(http.MethodPost, "/api/k8s/{path...}", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", user))
	observer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusOK)
	})).ServeHTTP(httptest.NewRecorder(), observedRequest(http.MethodGet, "/api/svc/{namespace}/{service}", "", user))

	if len(sink.events) != 1 {
		t.Fatalf("events = %#v", sink.events)
	}
	event := sink.events[0]
	if event.Name != EventLoginSucceeded || event.DistinctID != user.ID || event.InsertID != "" {
		t.Fatalf("event = %#v", event)
	}
	if event.Properties["outcome"] != OutcomeSuccess || event.Properties["source"] != SourceCLI || event.Properties["identity_class"] != IdentityExternal {
		t.Fatalf("properties = %#v", event.Properties)
	}
}

func TestLoginObserverCapturesDifferentCLISessions(t *testing.T) {
	sink := &captureSink{}
	handler := LoginObserver(sink, "cyclops-cs-spa")(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	for _, sessionID := range []string{"session-1", "session-2"} {
		user := &auth.User{ID: "external-1", AZP: "cua-cli", PrincipalType: auth.PrincipalTypeUser, Claims: map[string]string{"sid": sessionID}}
		handler.ServeHTTP(httptest.NewRecorder(), observedRequest(http.MethodGet, "/api/k8s/{path...}", "version", user))
	}
	if len(sink.events) != 2 {
		t.Fatalf("events = %#v", sink.events)
	}
}

func TestLoginSessionGateIsBoundedAndExpires(t *testing.T) {
	gate := newLoginSessionGate(1, time.Hour)
	now := time.Date(2026, time.September, 2, 0, 0, 0, 0, time.UTC)
	if !gate.first("session-1", now) || gate.first("session-1", now) {
		t.Fatal("same live session was not suppressed")
	}
	if !gate.first("session-2", now) || !gate.first("session-1", now) {
		t.Fatal("capacity eviction did not admit the oldest session key")
	}
	if !gate.first("session-1", now.Add(2*time.Hour)) {
		t.Fatal("expired session key remained suppressed")
	}
}

func TestLoginObserverCapturesBeforeTheFleetRequest(t *testing.T) {
	order := []string{}
	sink := captureFunc(func(Event) { order = append(order, "login") })
	handler := LoginObserver(sink, "cyclops-cs-spa")(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		order = append(order, "request")
		w.WriteHeader(http.StatusOK)
	}))
	handler.ServeHTTP(httptest.NewRecorder(), observedRequest(http.MethodGet, "/api/k8s/{path...}", "version", &auth.User{ID: "cli-1", AZP: "cua-cli", PrincipalType: auth.PrincipalTypeUser}))
	if got := strings.Join(order, ","); got != "login,request" {
		t.Fatalf("order = %q", got)
	}
}

func TestLoginObserverIgnoresBrowserAndNonInteractivePrincipals(t *testing.T) {
	for _, user := range []*auth.User{
		{ID: "spa-1", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser},
		{ID: "key-1", AZP: "ukey-demo", PrincipalType: auth.PrincipalTypeUserKey},
		{ID: "github-1", AZP: "github-oidc", PrincipalType: auth.PrincipalTypeGitHubOIDC},
		{ID: "github-2", AZP: "cua-cli", PrincipalType: auth.PrincipalTypeGitHubOIDC},
	} {
		sink := &captureSink{}
		handler := LoginObserver(sink, "cyclops-cs-spa")(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(http.StatusOK)
		}))
		handler.ServeHTTP(httptest.NewRecorder(), observedRequest(http.MethodGet, "/api/k8s/{path...}", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", user))
		if len(sink.events) != 0 {
			t.Fatalf("user %#v captured %#v", user, sink.events)
		}
	}
}

func TestRouteObserverEmitsActivationForAuthenticatedNonInternalIdentity(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-owner"]`)
	auth.InvalidateFeatureFlags()
	t.Cleanup(auth.InvalidateFeatureFlags)
	users := []struct {
		name string
		user *auth.User
		want int
	}{
		{name: "verified external", user: &auth.User{ID: "external-1", Email: "person@example.test", EmailVerified: true, AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, want: 3},
		{name: "missing email", user: &auth.User{ID: "external-2", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, want: 3},
		{name: "missing email user key", user: &auth.User{ID: "external-3", AZP: "ukey-proof", PrincipalType: auth.PrincipalTypeUserKey}, want: 3},
		{name: "unverified internal-looking email", user: &auth.User{ID: "external-4", Email: "person@trycua.com", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, want: 3},
		{name: "internal", user: &auth.User{ID: "internal-1", Email: "person@trycua.com", EmailVerified: true, AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, want: 1},
		{name: "admin SPA without email", user: &auth.User{ID: "admin-owner", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, want: 1},
		{name: "admin CLI without email", user: &auth.User{ID: "admin-owner", AZP: "cua-cli", PrincipalType: auth.PrincipalTypeUser}, want: 1},
		{name: "admin SDK owner without email", user: &auth.User{ID: "admin-owner", AZP: "ukey-proof", PrincipalType: auth.PrincipalTypeUserKey}, want: 1},
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
				if activation == nil || activation.InsertID != "" || activation.SetOnce[firstActivationProperty] == nil {
					t.Fatalf("activation event = %#v", activation)
				}
			} else {
				if events[0].Name != EventHTTPProxyRequest || events[0].Properties["identity_class"] != IdentityInternal {
					t.Fatalf("internal traffic must remain diagnostic only: %#v", events)
				}
				select {
				case event := <-sink:
					t.Fatalf("internal traffic emitted activation/workload: %#v", event)
				case <-time.After(30 * time.Millisecond):
				}
			}
		})
	}
}

func TestUnresolvedAdminMembershipNeverStartsWorkloadQualification(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `invalid`)
	auth.InvalidateFeatureFlags()
	t.Cleanup(auth.InvalidateFeatureFlags)
	sink := make(eventChannel, 3)
	started := make(chan struct{}, 1)
	handler := RouteObserver("/api/svc/{namespace}/{service}/{path...}", sink, "cyclops-cs-spa", func(context.Context, *http.Request, int) bool {
		started <- struct{}{}
		return true
	})(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusOK) }))
	user := &auth.User{ID: "owner", AZP: "cua-cli", PrincipalType: auth.PrincipalTypeUser}
	handler.ServeHTTP(httptest.NewRecorder(), observedRequest(http.MethodPost, "/api/svc/{namespace}/{service}/{path...}", "tools", user))
	event := <-sink
	if event.Name != EventHTTPProxyRequest || event.Properties["identity_class"] != IdentityUnknown {
		t.Fatalf("unresolved identity counted as external: %#v", event)
	}
	select {
	case <-started:
		t.Fatal("unresolved identity started qualification")
	case <-time.After(30 * time.Millisecond):
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
			wantEvents := 1
			if (test.status < 200 || test.status >= 300) && test.wantEvent != EventHTTPProxyRequest {
				wantEvents = 2
			}
			if len(sink.events) != wantEvents {
				t.Fatalf("events = %#v", sink.events)
			}
			event := sink.events[0]
			if event.Name != test.wantEvent || event.InsertID != "11111111111111111111111111111111" {
				t.Fatalf("event = %#v", event)
			}
			if event.Properties["outcome"] != test.wantOutcome || event.Properties["source"] != test.wantSource || event.Properties["error_class"] != test.wantClass {
				t.Fatalf("properties = %#v", event.Properties)
			}
			if wantEvents == 2 {
				blocked := sink.events[1]
				if blocked.Name != EventResourceBlocked || blocked.Properties["resource_type"] == "" || blocked.Properties["reason"] == "" {
					t.Fatalf("blocked event = %#v", blocked)
				}
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

func TestRouteObserverClassifiesPaymentRequiredWithoutChangingResponse(t *testing.T) {
	sink := &captureSink{}
	handler := RouteObserver("/api/k8s/{path...}", sink, "cyclops-cs-spa")(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusForbidden)
		_, _ = w.Write([]byte(`{"message":"` + auth.BillingSetupRequiredMessage + `"}`))
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, observedRequest(http.MethodPost, "/api/k8s/{path...}", "apis/cua.ai/v1/namespaces/ns-a/osgymworkspacepools", &auth.User{ID: "u-1", AZP: "cyclops-cs-spa"}))
	if response.Code != http.StatusForbidden || !bytes.Contains(response.Body.Bytes(), []byte("A payment method is required")) {
		t.Fatalf("response changed: status=%d body=%q", response.Code, response.Body.String())
	}
	if len(sink.events) != 2 || sink.events[1].Name != EventResourceBlocked || sink.events[1].Properties["reason"] != "payment_required" {
		t.Fatalf("events = %#v", sink.events)
	}
}

func TestRouteObserverEmitsBoundedQualificationRejectionForExternalUsers(t *testing.T) {
	for _, result := range []SvcQualificationResult{
		{Reason: "claim_mismatch"},
		{Reason: "binding_lookup_failed", LookupStage: "claim", LookupErrorClass: "deadline_exceeded"},
		{Reason: "pool_lookup_failed", LookupStage: "pool", LookupErrorClass: "forbidden"},
	} {
		t.Run(result.Reason, func(t *testing.T) {
			sink := make(eventChannel, 4)
			handler := RouteObserverWithQualification("/api/svc/{namespace}/{service}/{path...}", sink, "cyclops-cs-spa", func(context.Context, *http.Request, int) SvcQualificationResult {
				return result
			})(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusOK) }))
			user := &auth.User{ID: "external-1", Email: "person@example.test", EmailVerified: true, AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}
			handler.ServeHTTP(httptest.NewRecorder(), observedRequest(http.MethodGet, "/api/svc/{namespace}/{service}/{path...}", "tools", user))
			events := map[string]Event{}
			deadline := time.After(time.Second)
			for i := 0; i < 2; i++ {
				select {
				case event := <-sink:
					events[event.Name] = event
				case <-deadline:
					t.Fatalf("events = %#v", events)
				}
			}
			rejected, ok := events[EventQualificationRejected]
			if !ok || rejected.Properties["reason"] != result.Reason || rejected.DistinctID != user.ID || events[EventHTTPProxyRequest].Properties["outcome"] != OutcomeSuccess {
				t.Fatalf("events = %#v", events)
			}
			stage, hasStage := rejected.Properties["qualification_lookup_stage"]
			class, hasClass := rejected.Properties["qualification_error_class"]
			if result.LookupStage == "" {
				if hasStage || hasClass {
					t.Fatal("non-lookup rejection acquired lookup diagnostics")
				}
			} else if stage != result.LookupStage || class != result.LookupErrorClass {
				t.Fatalf("lookup diagnostics lost: %#v", rejected.Properties)
			}
			if err := ValidateEvent(rejected); err != nil {
				t.Fatal(err)
			}
		})
	}
}

func TestLookupDiagnosticDeliveryIsPrivateAndNeverEmitsActivation(t *testing.T) {
	var mu sync.Mutex
	var payloads []map[string]any
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var payload map[string]any
		if err := json.NewDecoder(r.Body).Decode(&payload); err != nil {
			t.Error(err)
		}
		mu.Lock()
		payloads = append(payloads, payload)
		mu.Unlock()
		w.WriteHeader(http.StatusOK)
	}))
	t.Cleanup(server.Close)
	client := New(Config{
		Enabled: true, Host: server.URL, ProjectToken: "phc_test", IdentityKey: "synthetic-key", Environment: "production",
		QueueSize: 4, BatchSize: 1, FlushInterval: time.Hour, RequestTimeout: time.Second,
	})
	t.Cleanup(func() {
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		defer cancel()
		if err := client.Shutdown(ctx); err != nil {
			t.Error(err)
		}
	})
	user := &auth.User{ID: "raw-synthetic-subject", Email: "person@example.test", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}
	result := SvcQualificationResult{Reason: "pool_lookup_failed", LookupStage: "pool", LookupErrorClass: "deadline_exceeded"}
	// Complete the synchronous producer, then flush before inspecting the whole
	// collection. A test reading only the first events can miss extra activations.
	captureSvcQualification(client, result, user, SourceSPA, http.StatusOK, "synthetic-trace")
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := client.Shutdown(ctx); err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	defer mu.Unlock()
	seen := map[string]int{}
	for _, payload := range payloads {
		encoded, _ := json.Marshal(payload)
		if bytes.Contains(encoded, []byte(user.ID)) || bytes.Contains(encoded, []byte(user.Email)) {
			t.Fatal("analytics payload exposed raw identity")
		}
		event := payload["batch"].([]any)[0].(map[string]any)
		name := event["event"].(string)
		seen[name]++
		if event["distinct_id"] != PseudonymForUserID(user.ID, "synthetic-key") {
			t.Fatal("analytics identity was not pseudonymized")
		}
		if name == EventQualificationRejected {
			properties := event["properties"].(map[string]any)
			if properties["status_code"] != float64(200) || properties["error_class"] != "" || properties["qualification_lookup_stage"] != "pool" || properties["qualification_error_class"] != "deadline_exceeded" {
				t.Fatalf("workload status or diagnostic changed: %#v", properties)
			}
		}
	}
	if seen[EventQualificationRejected] != 1 || len(seen) != 1 {
		t.Fatalf("lookup failure must not emit workload success or activation: %#v", seen)
	}
}
