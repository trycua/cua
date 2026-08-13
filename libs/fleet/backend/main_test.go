package main

import (
	"bytes"
	"context"
	"crypto/hmac"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"slices"
	"strings"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/billing"
	"cyclops-cs-backend/config"
	"cyclops-cs-backend/githubtrust"
	"cyclops-cs-backend/handlers"
	"cyclops-cs-backend/statequery"

	jwt "github.com/golang-jwt/jwt/v5"
	"github.com/trycua/cloud/pkg/featureflags"
)

func TestGatewayRoutesAreRemoved(t *testing.T) {
	router := setupRouter(handlers.Handlers{})

	for _, path := range []string{"/api/gateway/mypool", "/api/gateway/mypool/step"} {
		t.Run(path, func(t *testing.T) {
			response := httptest.NewRecorder()
			router.ServeHTTP(response, httptest.NewRequest(http.MethodGet, path, nil))
			if response.Code != http.StatusNotFound {
				t.Fatalf("status = %d, want 404; body = %s", response.Code, response.Body.String())
			}
		})
	}
}

func TestHealthAndReadinessRoutes(t *testing.T) {
	router := setupRouter(handlers.Handlers{})

	for _, test := range []struct {
		path       string
		statusCode int
	}{
		{path: "/healthz", statusCode: http.StatusOK},
		{path: "/readyz", statusCode: http.StatusOK},
	} {
		t.Run(test.path, func(t *testing.T) {
			response := httptest.NewRecorder()
			router.ServeHTTP(response, httptest.NewRequest(http.MethodGet, test.path, nil))

			if response.Code != test.statusCode {
				t.Fatalf("status = %d, want %d", response.Code, test.statusCode)
			}
		})
	}
}

func TestSwaggerOmitsGatewayRoute(t *testing.T) {
	data, err := os.ReadFile("docs/swagger.json")
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(data), `"/api/gateway/`) {
		t.Fatal("swagger.json still exposes the removed gateway route")
	}
}

func TestSwaggerUsesBillingSetupSessionRoute(t *testing.T) {
	data, err := os.ReadFile("docs/swagger.json")
	if err != nil {
		t.Fatal(err)
	}

	var spec struct {
		Paths map[string]json.RawMessage `json:"paths"`
	}
	if err := json.Unmarshal(data, &spec); err != nil {
		t.Fatalf("unmarshal swagger.json: %v", err)
	}
	if _, ok := spec.Paths["/api/billing/setup-session"]; !ok {
		t.Fatal("swagger.json missing /api/billing/setup-session")
	}
	if _, ok := spec.Paths["/api/billing/checkout-session"]; ok {
		t.Fatal("swagger.json still contains /api/billing/checkout-session")
	}
}

// TestBatchAndLabelRoutesAreRemoved pins the client-visible consequence of
// deleting the surface: these paths used to answer 410 Gone with an explanatory
// body, and now fall through to the mux's bare 404. Every path is exercised
// rather than a representative one, so a route restored by accident is visible
// by name.
func TestBatchAndLabelRoutesAreRemoved(t *testing.T) {
	router := setupRouter(handlers.Handlers{})

	cases := []struct {
		method string
		path   string
		body   io.Reader
	}{
		// batch routes
		{http.MethodPost, "/api/batch/demo/submit", strings.NewReader(`{"runs":[]}`)},
		{http.MethodPost, "/api/batch/demo/lanes", strings.NewReader(`{}`)},
		{http.MethodDelete, "/api/batch/demo/lanes", nil},
		{http.MethodGet, "/api/batch/demo/run-1/status", nil},
		{http.MethodGet, "/api/batch/demo/run-1/results", nil},
		{http.MethodDelete, "/api/batch/demo/run-1", nil},
		// label routes
		{http.MethodPost, "/api/label/demo/run-1/batch", strings.NewReader(`{}`)},
		{http.MethodGet, "/api/label/demo/run-1/status", nil},
		{http.MethodGet, "/api/label/demo/run-1/results", nil},
		{http.MethodDelete, "/api/label/demo/run-1", nil},
	}

	for _, tc := range cases {
		t.Run(tc.method+" "+tc.path, func(t *testing.T) {
			req := authorizedRequest(t, tc.method, tc.path, tc.body)
			w := httptest.NewRecorder()
			router.ServeHTTP(w, req)

			if w.Code != http.StatusNotFound {
				t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusNotFound, w.Body.String())
			}
		})
	}
}

// TestSwaggerOmitsBatchAndLabelRoutes is the generated-artifact half of the
// same removal: `swag init` reads annotations off handlers, so a wrapper left
// behind keeps documenting a route the router no longer serves.
func TestSwaggerOmitsBatchAndLabelRoutes(t *testing.T) {
	data, err := os.ReadFile("docs/swagger.json")
	if err != nil {
		t.Fatal(err)
	}
	body := string(data)
	for _, prefix := range []string{`"/api/batch/`, `"/api/label/`} {
		if strings.Contains(body, prefix) {
			t.Fatalf("swagger.json still exposes the removed %s routes", strings.Trim(prefix, `"`))
		}
	}
}

const (
	testIssuer = "https://issuer.example.test/realms/cyclops-cs"
	testKeyID  = "router-test-key"
)

var testSigningKey *rsa.PrivateKey

func TestMain(m *testing.M) {
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		panic(err)
	}
	testSigningKey = key

	jwksBody, err := json.Marshal(map[string]any{
		"keys": []map[string]any{{
			"kty": "RSA",
			"use": "sig",
			"alg": "RS256",
			"kid": testKeyID,
			"n":   base64.RawURLEncoding.EncodeToString(key.N.Bytes()),
			"e":   base64.RawURLEncoding.EncodeToString(bigEndianBytes(key.E)),
		}},
	})
	if err != nil {
		panic(err)
	}

	jwksServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write(jwksBody)
	}))
	defer jwksServer.Close()

	if err := os.Setenv("CYCLOPS_CS_ADMIN_SUBS", `[]`); err != nil {
		panic(err)
	}
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		panic(err)
	}
	auth.LoadOpa()
	if err := auth.Init(&config.AuthConfiguration{
		Issuer:           testIssuer,
		JWKSUri:          jwksServer.URL,
		SigningAlgs:      []string{"RS256"},
		SPAClientID:      "cyclops-cs-spa",
		KeyClientPfx:     "key-",
		UserKeyClientPfx: "ukey-",
	}); err != nil {
		panic(err)
	}

	os.Exit(m.Run())
}

func authorizedRequest(t *testing.T, method, path string, body io.Reader) *http.Request {
	return requestWithAZP(t, method, path, body, "cyclops-cs-spa")
}

func requestWithAZP(t *testing.T, method, path string, body io.Reader, azp string) *http.Request {
	t.Helper()

	req := httptest.NewRequest(method, path, body)
	token := jwt.NewWithClaims(jwt.SigningMethodRS256, jwt.MapClaims{
		"iss": testIssuer,
		"sub": "user-123",
		"azp": azp,
		"exp": time.Now().Add(time.Hour).Unix(),
		"iat": time.Now().Add(-time.Minute).Unix(),
	})
	token.Header["kid"] = testKeyID

	raw, err := token.SignedString(testSigningKey)
	if err != nil {
		t.Fatalf("sign test token: %v", err)
	}
	req.Header.Set("Authorization", "Bearer "+raw)
	return req
}

func bigEndianBytes(v int) []byte {
	if v == 0 {
		return []byte{0}
	}
	var out []byte
	for n := v; n > 0; n >>= 8 {
		out = append([]byte{byte(n & 0xff)}, out...)
	}
	return out
}

type routerBillingService struct{}

func (routerBillingService) Summary(context.Context, string) (billing.Summary, error) {
	return billing.Summary{}, nil
}

func (routerBillingService) CreateSetupSession(context.Context, string, billing.SetupOptions) (string, error) {
	return "https://checkout.stripe.test/session", nil
}

func (routerBillingService) CreatePortalSession(context.Context, string, string) (string, error) {
	return "https://billing.stripe.test/session", nil
}

func (routerBillingService) SetDefaultPaymentMethodForSetupGeneration(context.Context, string, string, string) (bool, error) {
	return true, nil
}

func TestBillingRouterAuthorizationBoundaries(t *testing.T) {
	t.Setenv("CYCLOPS_CS_BILLING_ENABLED", "true")
	const webhookSecret = "whsec_router_test"
	h := handlers.Handlers{
		Billing:         routerBillingService{},
		WebhookVerifier: billing.NewStripeWebhookVerifier(),
		Stripe: config.StripeConfiguration{
			SecretKey:     "sk_test",
			WebhookSecret: webhookSecret,
		},
	}
	router := setupRouter(h)

	unauthorized := httptest.NewRecorder()
	router.ServeHTTP(unauthorized, httptest.NewRequest(http.MethodGet, "/api/billing/summary", nil))
	if unauthorized.Code != http.StatusUnauthorized {
		t.Fatalf("unauthenticated summary status = %d, want 401; body = %s", unauthorized.Code, unauthorized.Body.String())
	}

	authorized := httptest.NewRecorder()
	router.ServeHTTP(authorized, authorizedRequest(t, http.MethodGet, "/api/billing/summary", nil))
	if authorized.Code != http.StatusOK {
		t.Fatalf("authenticated summary status = %d, want 200; body = %s", authorized.Code, authorized.Body.String())
	}

	for _, tc := range []struct {
		name string
		path string
	}{
		{name: "setup", path: "/api/billing/setup-session"},
		{name: "portal", path: "/api/billing/portal-session"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			unauthenticated := httptest.NewRecorder()
			router.ServeHTTP(unauthenticated, httptest.NewRequest(http.MethodPost, tc.path, nil))
			if unauthenticated.Code != http.StatusUnauthorized {
				t.Fatalf("unauthenticated status = %d, want 401; body = %s", unauthenticated.Code, unauthenticated.Body.String())
			}

			nonSPA := httptest.NewRecorder()
			router.ServeHTTP(nonSPA, requestWithAZP(t, http.MethodPost, tc.path, nil, "some-other-client"))
			if nonSPA.Code != http.StatusForbidden {
				t.Fatalf("non-SPA status = %d, want 403; body = %s", nonSPA.Code, nonSPA.Body.String())
			}
		})
	}

	removedCheckout := httptest.NewRecorder()
	router.ServeHTTP(removedCheckout, authorizedRequest(t, http.MethodPost, "/api/billing/checkout-session", nil))
	if removedCheckout.Code != http.StatusNotFound {
		t.Fatalf("removed checkout-session status = %d, want 404; body = %s", removedCheckout.Code, removedCheckout.Body.String())
	}

	payload := []byte(`{"id":"evt_router","type":"setup_intent.succeeded"}`)
	timestamp := time.Now().Unix()
	mac := hmac.New(sha256.New, []byte(webhookSecret))
	_, _ = fmt.Fprintf(mac, "%d.", timestamp)
	_, _ = mac.Write(payload)
	signature := fmt.Sprintf("t=%d,v1=%s", timestamp, hex.EncodeToString(mac.Sum(nil)))
	webhookRequest := httptest.NewRequest(http.MethodPost, "/api/billing/webhook", bytes.NewReader(payload))
	webhookRequest.Header.Set("Stripe-Signature", signature)
	webhookResponse := httptest.NewRecorder()
	router.ServeHTTP(webhookResponse, webhookRequest)
	if webhookResponse.Code != http.StatusNoContent {
		t.Fatalf("signed webhook status = %d, want 204; body = %s", webhookResponse.Code, webhookResponse.Body.String())
	}
}

func TestBillingSummaryGeneratedContract(t *testing.T) {
	data, err := os.ReadFile("docs/swagger.json")
	if err != nil {
		t.Fatalf("read swagger.json: %v", err)
	}
	var document struct {
		Definitions map[string]struct {
			Required   []string                   `json:"required"`
			Properties map[string]json.RawMessage `json:"properties"`
		} `json:"definitions"`
	}
	if err := json.Unmarshal(data, &document); err != nil {
		t.Fatalf("unmarshal swagger.json: %v", err)
	}
	summary := document.Definitions["billing.Summary"]
	if !slices.Contains(summary.Required, "payment_method_present") || !slices.Contains(summary.Required, "card") {
		t.Fatalf("billing summary required fields = %#v", summary.Required)
	}
	var card struct {
		Nullable bool `json:"x-nullable"`
	}
	if err := json.Unmarshal(summary.Properties["card"], &card); err != nil {
		t.Fatalf("unmarshal card property: %v", err)
	}
	if !card.Nullable {
		t.Fatal("billing summary card must be explicitly nullable")
	}
}

func TestStateQueryRouterUsesQueryMethod(t *testing.T) {
	router := setupRouter(handlers.Handlers{})

	queryRequest := authorizedRequest(t, "QUERY", "/api/state/query", strings.NewReader("select 1"))
	queryRequest.Header.Set("Content-Type", "application/sql")
	queryResponse := httptest.NewRecorder()
	router.ServeHTTP(queryResponse, queryRequest)
	if queryResponse.Code != http.StatusServiceUnavailable {
		t.Fatalf("QUERY status = %d, want 503; body = %s", queryResponse.Code, queryResponse.Body.String())
	}

	postResponse := httptest.NewRecorder()
	router.ServeHTTP(postResponse, authorizedRequest(t, http.MethodPost, "/api/state/query", strings.NewReader(`{"sql":"select 1"}`)))
	if postResponse.Code != http.StatusMethodNotAllowed {
		t.Fatalf("POST status = %d, want 405; body = %s", postResponse.Code, postResponse.Body.String())
	}
	if got := postResponse.Header().Get("Allow"); got != "QUERY" {
		t.Fatalf("Allow = %q, want QUERY", got)
	}
}

func TestWithMiddlewaresAppliesInDeclarationOrder(t *testing.T) {
	var order []string
	middleware := func(name string) auth.Middleware {
		return func(next http.Handler) http.Handler {
			return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				order = append(order, name+":before")
				next.ServeHTTP(w, r)
				order = append(order, name+":after")
			})
		}
	}
	handler := withMiddlewares(
		http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			order = append(order, "handler")
			w.WriteHeader(http.StatusNoContent)
		}),
		middleware("first"),
		middleware("second"),
	)
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/", nil))

	want := []string{"first:before", "second:before", "handler", "second:after", "first:after"}
	if !slices.Equal(order, want) {
		t.Fatalf("order = %v, want %v", order, want)
	}
}

func TestK8sRouteRejectsDisallowedPoolBeforeProxy(t *testing.T) {
	upstreamCalled := false
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		upstreamCalled = true
		w.WriteHeader(http.StatusCreated)
	}))
	defer upstream.Close()
	t.Setenv("KUBECTL_PROXY_ADDR", upstream.URL)

	router := setupRouter(handlers.Handlers{})
	body := strings.NewReader(`{"spec":{"template":{"containerDiskImage":"evil.example/workspace:latest","imagePullSecret":"ecr-credentials"}}}`)
	request := authorizedRequest(t, http.MethodPost, "/api/k8s/apis/cua.ai/v1/namespaces/foo/osgymworkspacepools", body)
	response := httptest.NewRecorder()
	router.ServeHTTP(response, request)

	if response.Code != http.StatusForbidden {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusForbidden, response.Body.String())
	}
	if upstreamCalled {
		t.Fatal("disallowed pool request reached kubectl proxy")
	}
}

func TestSwaggerIncludesChatRoutes(t *testing.T) {
	data, err := os.ReadFile("docs/swagger.json")
	if err != nil {
		t.Fatal(err)
	}
	var spec struct {
		Paths map[string]map[string]struct{} `json:"paths"`
	}
	if err := json.Unmarshal(data, &spec); err != nil {
		t.Fatalf("unmarshal swagger.json: %v", err)
	}
	for _, tc := range []struct{ path, method string }{
		{"/api/chat/conversations", "post"}, {"/api/chat/conversations", "get"},
		{"/api/chat/conversations/{id}", "get"}, {"/api/chat/conversations/{id}/turns", "post"},
	} {
		if _, ok := spec.Paths[tc.path][tc.method]; !ok {
			t.Fatalf("swagger.json missing %s %s", strings.ToUpper(tc.method), tc.path)
		}
	}
}

func TestNginxRoutesChatToBackend(t *testing.T) {
	data, err := os.ReadFile("../nginx.conf")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(data), "|chat|") && !strings.Contains(string(data), "|chat)") {
		t.Fatal("nginx.conf backend matcher does not include chat")
	}
}

func TestInitializeDatabaseFeatures(t *testing.T) {
	tests := []struct {
		name                string
		config              config.DatabaseConfiguration
		requireVersionError error
		newExecutorError    error
		newStoreError       error
		wantExecutor        bool
		wantStore           bool
		wantRequireVersion  bool
		wantNewExecutor     bool
		wantNewStore        bool
	}{
		{
			name: "state query initializes without application database",
			config: config.DatabaseConfiguration{
				StateQueryDSN:            "postgres://state-query/state-query",
				StateQueryTenantPassword: "tenant-password",
			},
			wantExecutor:    true,
			wantNewExecutor: true,
		},
		{
			name: "state query initializes when application schema validation fails",
			config: config.DatabaseConfiguration{
				URL:                      "postgres://application/application",
				StateQueryDSN:            "postgres://state-query/state-query",
				StateQueryTenantPassword: "tenant-password",
			},
			requireVersionError: fmt.Errorf("schema unavailable"),
			wantExecutor:        true,
			wantRequireVersion:  true,
			wantNewExecutor:     true,
		},
		{
			name:               "application database enables GitHub trust store",
			config:             config.DatabaseConfiguration{URL: "postgres://application/application"},
			wantStore:          true,
			wantRequireVersion: true,
			wantNewStore:       true,
		},
		{
			name: "constructor failures are non-fatal and clear stale fields",
			config: config.DatabaseConfiguration{
				URL:                      "postgres://application/application",
				StateQueryDSN:            "postgres://state-query/state-query",
				StateQueryTenantPassword: "tenant-password",
			},
			newExecutorError:   fmt.Errorf("invalid state query configuration"),
			newStoreError:      fmt.Errorf("store unavailable"),
			wantRequireVersion: true,
			wantNewExecutor:    true,
			wantNewStore:       true,
		},
		{
			name:               "missing database config clears stale fields",
			config:             config.DatabaseConfiguration{},
			wantRequireVersion: false,
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			var requireVersionContext context.Context
			var storeContext context.Context
			newExecutorCalls := 0
			newStoreCalls := 0
			dependencies := databaseFeatureDependencies{
				requireVersion: func(ctx context.Context, _ string, _ int64) error {
					requireVersionContext = ctx
					return test.requireVersionError
				},
				newStateQueryExecutor: func(_ string, _ string) (handlers.StateQueryExecutor, error) {
					newExecutorCalls++
					if test.newExecutorError != nil {
						return nil, test.newExecutorError
					}
					return stateQueryExecutorStub{}, nil
				},
				newGitHubTrustStore: func(ctx context.Context, _ string) (githubtrust.Store, error) {
					newStoreCalls++
					storeContext = ctx
					if test.newStoreError != nil {
						return nil, test.newStoreError
					}
					return githubTrustStoreStub{}, nil
				},
			}
			h := handlers.Handlers{
				StateQueryExecutor:  stateQueryExecutorStub{},
				GitHubTrustPolicies: githubTrustStoreStub{},
			}
			auth.SetGitHubTrustResolver(handlers.NewGitHubTrustResolver(githubTrustStoreStub{}))
			t.Cleanup(func() { auth.SetGitHubTrustResolver(nil) })

			initializeDatabaseFeatures(context.Background(), test.config, &h, dependencies)

			if got := newExecutorCalls == 1; got != test.wantNewExecutor {
				t.Fatalf("state-query constructor called = %t, want %t", got, test.wantNewExecutor)
			}
			if got := newStoreCalls == 1; got != test.wantNewStore {
				t.Fatalf("GitHub trust store constructor called = %t, want %t", got, test.wantNewStore)
			}
			if got := h.StateQueryExecutor != nil; got != test.wantExecutor {
				t.Fatalf("StateQueryExecutor present = %t, want %t", got, test.wantExecutor)
			}
			if got := h.GitHubTrustPolicies != nil; got != test.wantStore {
				t.Fatalf("GitHubTrustPolicies present = %t, want %t", got, test.wantStore)
			}
			assertStartupContext(t, requireVersionContext, test.wantRequireVersion)
			assertStartupContext(t, storeContext, test.wantNewStore)
		})
	}
}

func assertStartupContext(t *testing.T, ctx context.Context, wantCalled bool) {
	t.Helper()
	if !wantCalled {
		if ctx != nil {
			t.Fatal("initializer received a context unexpectedly")
		}
		return
	}
	if ctx == nil {
		t.Fatal("initializer did not receive a context")
	}
	deadline, ok := ctx.Deadline()
	if !ok {
		t.Fatal("initializer context has no deadline")
	}
	if remaining := time.Until(deadline); remaining > databaseStartupTimeout+100*time.Millisecond {
		t.Fatalf("context deadline is too far away: %s", remaining)
	}
	if err := ctx.Err(); err != context.Canceled {
		t.Fatalf("initializer context error = %v, want %v", err, context.Canceled)
	}
}

type stateQueryExecutorStub struct{}

func (stateQueryExecutorStub) Execute(context.Context, string, string, statequery.ResultWriter) error {
	return nil
}

type githubTrustStoreStub struct{}

func (githubTrustStoreStub) List(context.Context, string) ([]*githubtrust.Policy, error) {
	return nil, nil
}
func (githubTrustStoreStub) Create(context.Context, *githubtrust.Policy) error { return nil }
func (githubTrustStoreStub) Get(context.Context, string, string) (*githubtrust.Policy, error) {
	return nil, nil
}
func (githubTrustStoreStub) Update(context.Context, *githubtrust.Policy) error { return nil }
func (githubTrustStoreStub) Delete(context.Context, string, string) (bool, error) {
	return false, nil
}
func (githubTrustStoreStub) ResolveByRepository(context.Context, string) ([]*githubtrust.Policy, error) {
	return nil, nil
}
