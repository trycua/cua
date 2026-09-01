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
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"reflect"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/billing"
	"cyclops-cs-backend/config"
	"cyclops-cs-backend/featureflagadmin"
	"cyclops-cs-backend/githubtrust"
	"cyclops-cs-backend/handlers"
	"cyclops-cs-backend/metrics"
	"cyclops-cs-backend/productanalytics"
	"cyclops-cs-backend/statequery"
	"cyclops-cs-backend/usage"

	"github.com/prometheus/client_golang/prometheus"
	dto "github.com/prometheus/client_model/go"

	jwt "github.com/golang-jwt/jwt/v5"
	"github.com/trycua/cloud/pkg/featureflags"
)

type auditWiringLock struct{}

func (auditWiringLock) WithLock(ctx context.Context, callback func(context.Context) error) error {
	return callback(ctx)
}

func TestFeatureFlagAdminServiceWiresMutationAuditLogger(t *testing.T) {
	var output bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewJSONHandler(&output, nil)))
	t.Cleanup(func() { slog.SetDefault(previous) })

	service := newFeatureFlagAdminService(nil, auditWiringLock{})
	_, err := service.Create(context.Background(), featureflagadmin.Actor{Subject: "admin-1"}, featureflagadmin.CreateInput{Key: "INVALID"})
	if err == nil {
		t.Fatal("Create() error = nil, want invalid key rejection")
	}
	if !strings.Contains(output.String(), `"event":"feature_flag_admin"`) || !strings.Contains(output.String(), `"reason":"invalid_key"`) {
		t.Fatalf("production mutation audit missing: %s", output.String())
	}
}

func TestUnsupportedFeatureFlagAdminServiceWiresMutationAuditLogger(t *testing.T) {
	var output bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewJSONHandler(&output, nil)))
	t.Cleanup(func() { slog.SetDefault(previous) })

	service := newUnsupportedFeatureFlagAdminService()
	_, err := service.Create(context.Background(), featureflagadmin.Actor{Subject: "admin-1"}, featureflagadmin.CreateInput{
		Key: "enabled", ValueType: featureflags.ValueBoolean, Value: true,
	})
	var serviceError *featureflagadmin.ServiceError
	if !errors.As(err, &serviceError) || serviceError.HTTPStatus != http.StatusNotImplemented {
		t.Fatalf("Create() error = %#v, want 501", err)
	}
	if !strings.Contains(output.String(), `"event":"feature_flag_admin"`) || !strings.Contains(output.String(), `"reason":"unsupported_provider"`) {
		t.Fatalf("fallback mutation audit missing: %s", output.String())
	}
}

func TestFeatureFlagAdminSwaggerDocumentsReachableResponses(t *testing.T) {
	data, err := os.ReadFile("docs/swagger.json")
	if err != nil {
		t.Fatal(err)
	}
	type swaggerResponse struct {
		Schema struct {
			Ref string `json:"$ref"`
		} `json:"schema"`
	}
	var document struct {
		Paths map[string]map[string]struct {
			Responses map[string]swaggerResponse `json:"responses"`
		} `json:"paths"`
	}
	if err := json.Unmarshal(data, &document); err != nil {
		t.Fatal(err)
	}
	wants := map[string]map[string][]string{
		"/api/admin/feature-flags":       {"get": {"200", "401", "403", "501", "502"}, "post": {"201", "400", "401", "403", "409", "422", "500", "501", "502", "503"}},
		"/api/admin/feature-flags/{key}": {"put": {"200", "400", "401", "403", "404", "409", "422", "500", "501", "502", "503"}, "delete": {"204", "400", "401", "403", "404", "409", "422", "500", "501", "502", "503"}},
	}
	for path, methods := range wants {
		for method, statuses := range methods {
			operation := document.Paths[path][method]
			for _, status := range statuses {
				response, ok := operation.Responses[status]
				if !ok {
					t.Errorf("%s %s missing Swagger response %s", method, path, status)
					continue
				}
				wantRef := "#/definitions/handlers.AdminAPIError"
				if status == "401" {
					wantRef = "#/definitions/handlers.ErrorResponse"
				}
				if status != "200" && status != "201" && status != "204" && response.Schema.Ref != wantRef {
					t.Errorf("%s %s response %s schema = %q, want %q", method, path, status, response.Schema.Ref, wantRef)
				}
			}
		}
	}
}

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

func TestOrchRouteIsRemoved(t *testing.T) {
	router := setupRouter(handlers.Handlers{})
	response := httptest.NewRecorder()
	router.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/api/orch/ns-a/catalog/items", nil))
	if response.Code != http.StatusNotFound {
		t.Fatalf("status = %d, want 404; body = %s", response.Code, response.Body.String())
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

func TestSwaggerOmitsOrchRoute(t *testing.T) {
	data, err := os.ReadFile("docs/swagger.json")
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(data), `"/api/orch/`) {
		t.Fatal("swagger.json still exposes the removed orch route")
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
	if _, ok := spec.Paths["/api/billing/usage"]; !ok {
		t.Fatal("swagger.json missing /api/billing/usage")
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

	if os.Getenv("CARD_ADMISSION_ROUTER_HELPER") != "1" {
		if err := os.Setenv("CYCLOPS_CS_REQUIRE_CARD_FOR_CUSTOM_RESOURCE_CREATION", "false"); err != nil {
			panic(err)
		}
	}
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

func (routerBillingService) AttachedCards(context.Context, string) ([]billing.SavedCard, error) {
	return []billing.SavedCard{}, nil
}

func (routerBillingService) Summary(context.Context, string) (billing.Summary, error) {
	return billing.Summary{}, nil
}

func (routerBillingService) Usage(context.Context, string, int, time.Time) (billing.Usage, error) {
	return billing.Usage{Currency: "usd", Trend: []billing.UsagePoint{}, Breakdown: []billing.UsageBreakdownItem{}}, nil
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

	unauthorizedUsage := httptest.NewRecorder()
	router.ServeHTTP(unauthorizedUsage, httptest.NewRequest(http.MethodGet, "/api/billing/usage", nil))
	if unauthorizedUsage.Code != http.StatusUnauthorized {
		t.Fatalf("unauthenticated usage status = %d, want 401; body = %s", unauthorizedUsage.Code, unauthorizedUsage.Body.String())
	}

	authorizedUsage := httptest.NewRecorder()
	router.ServeHTTP(authorizedUsage, authorizedRequest(t, http.MethodGet, "/api/billing/usage", nil))
	if authorizedUsage.Code != http.StatusOK {
		t.Fatalf("authenticated usage status = %d, want 200; body = %s", authorizedUsage.Code, authorizedUsage.Body.String())
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
	if !slices.Contains(summary.Required, "payment_method_present") || !slices.Contains(summary.Required, "card") || !slices.Contains(summary.Required, "pool_create_card_required") {
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

func TestChatConversationRouteRegistersPatch(t *testing.T) {
	router := setupRouter(handlers.Handlers{})
	request := httptest.NewRequest(http.MethodPatch, "/api/chat/conversations/id-1", nil)
	response := httptest.NewRecorder()
	router.ServeHTTP(response, request)
	if response.Code != http.StatusUnauthorized {
		t.Fatalf("PATCH status = %d, want 401; body = %s", response.Code, response.Body.String())
	}
}

func TestSwaggerIncludesChatRoutes(t *testing.T) {
	data, err := os.ReadFile("docs/swagger.json")
	if err != nil {
		t.Fatal(err)
	}
	type swaggerParameter struct {
		In   string `json:"in"`
		Name string `json:"name"`
	}
	type swaggerOperation struct {
		Parameters []swaggerParameter `json:"parameters"`
	}
	type swaggerSchema struct {
		Required []string `json:"required"`
	}
	var spec struct {
		Definitions map[string]swaggerSchema               `json:"definitions"`
		Paths       map[string]map[string]swaggerOperation `json:"paths"`
	}
	if err := json.Unmarshal(data, &spec); err != nil {
		t.Fatalf("unmarshal swagger.json: %v", err)
	}
	for _, tc := range []struct{ path, method string }{
		{"/api/chat/conversations", "post"}, {"/api/chat/conversations", "get"},
		{"/api/chat/conversations/{id}", "get"}, {"/api/chat/conversations/{id}", "patch"},
		{"/api/chat/conversations/{id}/turns", "post"},
	} {
		if _, ok := spec.Paths[tc.path][tc.method]; !ok {
			t.Fatalf("swagger.json missing %s %s", strings.ToUpper(tc.method), tc.path)
		}
	}
	archivedQueryFound := false
	for _, parameter := range spec.Paths["/api/chat/conversations"]["get"].Parameters {
		if parameter.In == "query" && parameter.Name == "archived" {
			archivedQueryFound = true
			break
		}
	}
	if !archivedQueryFound {
		t.Fatal("swagger.json GET /api/chat/conversations missing archived query parameter")
	}
	if !slices.Contains(spec.Definitions["handlers.ArchiveConversationRequest"].Required, "archived") {
		t.Fatal("swagger.json ArchiveConversationRequest missing required archived field")
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

type routerCardAccounts struct{}

func (routerCardAccounts) UserCreatedAt(context.Context, string) (time.Time, error) {
	return time.Date(2026, time.August, 15, 0, 0, 0, 0, time.UTC), nil
}

type routerCardBilling struct {
	cards []billing.SavedCard
	err   error
	calls int
}

func (service *routerCardBilling) AttachedCards(context.Context, string) ([]billing.SavedCard, error) {
	service.calls++
	return service.cards, service.err
}
func (*routerCardBilling) Summary(context.Context, string) (billing.Summary, error) {
	panic("unexpected Summary call")
}
func (*routerCardBilling) CreateSetupSession(context.Context, string, billing.SetupOptions) (string, error) {
	panic("unexpected setup call")
}
func (*routerCardBilling) CreatePortalSession(context.Context, string, string) (string, error) {
	panic("unexpected portal call")
}
func (*routerCardBilling) SetDefaultPaymentMethodForSetupGeneration(context.Context, string, string, string) (bool, error) {
	panic("unexpected webhook call")
}

func TestK8sRouteRejectsCustomResourceCreateWithoutCardBeforeProxy(t *testing.T) {
	if os.Getenv("CARD_ADMISSION_ROUTER_HELPER") != "1" {
		command := exec.Command(os.Args[0], "-test.run=^TestK8sRouteRejectsCustomResourceCreateWithoutCardBeforeProxy$")
		command.Env = append(os.Environ(),
			"CARD_ADMISSION_ROUTER_HELPER=1",
			"CYCLOPS_CS_REQUIRE_CARD_FOR_CUSTOM_RESOURCE_CREATION=true",
		)
		output, err := command.CombinedOutput()
		if err != nil {
			t.Fatalf("enabled router subprocess failed: %v\n%s", err, output)
		}
		return
	}

	upstreamCalled := false
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		upstreamCalled = true
		w.WriteHeader(http.StatusCreated)
	}))
	defer upstream.Close()
	t.Setenv("KUBECTL_PROXY_ADDR", upstream.URL)

	service := &routerCardBilling{}
	router := setupRouter(handlers.Handlers{Billing: service, UserAccounts: routerCardAccounts{}})
	cases := []struct {
		name string
		path string
		body string
	}{
		{
			name: "legacy workspace pool",
			path: "/api/k8s/apis/cua.ai/v1/namespaces/foo/osgymworkspacepools",
			body: `{"spec":{"template":{"containerDiskImage":"public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:latest","imagePullSecret":"ecr-credentials"}}}`,
		},
		{
			name: "native sandbox template",
			path: "/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/foo/osgymsandboxtemplates",
			body: `{"spec":{"vmTemplate":{"containerDiskImage":"public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:latest","imagePullSecret":"ecr-credentials"}}}`,
		},
	}
	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			response := httptest.NewRecorder()
			router.ServeHTTP(response, authorizedRequest(t, http.MethodPost, testCase.path, strings.NewReader(testCase.body)))
			if response.Code != http.StatusForbidden {
				t.Fatalf("status = %d, want 403; body = %s", response.Code, response.Body.String())
			}
		})
	}
	if upstreamCalled {
		t.Fatal("denied custom-resource create reached kubectl proxy")
	}

	service.cards = []billing.SavedCard{{ExpYear: 2099, ExpMonth: 1}}
	response := httptest.NewRecorder()
	router.ServeHTTP(response, authorizedRequest(t, http.MethodPost, cases[0].path, strings.NewReader(cases[0].body)))
	if response.Code != http.StatusCreated {
		t.Fatalf("valid attached card status = %d, want 201; body = %s", response.Code, response.Body.String())
	}
	if !upstreamCalled {
		t.Fatal("allowed custom-resource create did not reach kubectl proxy")
	}
	if service.calls != len(cases)+1 {
		t.Fatalf("Stripe attached-card calls = %d, want %d", service.calls, len(cases)+1)
	}
}

func TestInitializeSignedServiceURLs_BaseOnlyConfigurationDisablesFeature(t *testing.T) {
	const baseURL = "https://run.cua.ai"

	var logged bytes.Buffer
	restore := slog.Default()
	slog.SetDefault(slog.New(slog.NewJSONHandler(&logged, nil)))
	t.Cleanup(func() { slog.SetDefault(restore) })

	service, closeStore, err := initializeSignedServiceURLs(context.Background(), "postgres://application@db.example/cyclops", config.SignedServiceURLConfiguration{BaseURL: baseURL})
	defer closeStore()
	if err != nil {
		t.Fatalf("initializeSignedServiceURLs() error = %v, want disabled feature", err)
	}
	if service != nil {
		t.Fatal("initializeSignedServiceURLs() service = non-nil, want disabled feature")
	}
	if got := logged.String(); !strings.Contains(got, "SIGNED_SERVICE_URL_SECRET") || strings.Contains(got, baseURL) || strings.Contains(got, "DATABASE_URL") {
		t.Fatalf("signed URL disabled log = %s, want only the missing secret key name", got)
	}
}

func TestInitializeSignedServiceURLs_RecoversWhenSecretFileMaterializes(t *testing.T) {
	const secret = "12345678901234567890123456789012"

	secretFile := t.TempDir() + "/hmac_secret"
	cfg := config.SignedServiceURLConfiguration{
		BaseURL:    "https://run.cua.ai",
		SecretFile: secretFile,
	}

	service, closeStore, err := initializeSignedServiceURLs(context.Background(), "", cfg)
	defer closeStore()
	if err == nil || !strings.Contains(err.Error(), "SIGNED_SERVICE_URL_SECRET_FILE") {
		t.Fatalf("initializeSignedServiceURLs() error = %v, want retryable missing secret file error", err)
	}
	if service != nil {
		t.Fatal("initializeSignedServiceURLs() service = non-nil with missing secret file")
	}
	if strings.Contains(err.Error(), secret) {
		t.Fatalf("missing secret file error exposed secret: %v", err)
	}

	if err := os.WriteFile(secretFile, []byte(secret), 0o600); err != nil {
		t.Fatal(err)
	}
	service, closeStore, err = initializeSignedServiceURLs(context.Background(), "", cfg)
	defer closeStore()
	if err != nil {
		t.Fatalf("initializeSignedServiceURLs() error after secret file materialized = %v", err)
	}
	if service != nil {
		t.Fatal("initializeSignedServiceURLs() service = non-nil without database configuration")
	}
}

func TestInitializeSignedServiceURLs_SecretFileValidation(t *testing.T) {
	const secret = "12345678901234567890123456789012"

	tests := []struct {
		name      string
		contents  string
		secret    string
		wantError string
	}{
		{name: "empty file", wantError: "SIGNED_SERVICE_URL_SECRET_FILE must not be empty"},
		{name: "whitespace-only file", contents: "\n \t\n", wantError: "SIGNED_SERVICE_URL_SECRET_FILE must not be empty"},
		{name: "short file", contents: "too-short\n", wantError: "SIGNED_SERVICE_URL_SECRET_FILE must be at least 32 bytes"},
		{name: "valid file trims trailing newline", contents: secret + "\n"},
		{name: "environment secret takes precedence over file", contents: "too-short", secret: secret},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			secretFile := t.TempDir() + "/hmac_secret"
			if err := os.WriteFile(secretFile, []byte(test.contents), 0o600); err != nil {
				t.Fatal(err)
			}

			service, closeStore, err := initializeSignedServiceURLs(context.Background(), "", config.SignedServiceURLConfiguration{
				BaseURL:    "https://run.cua.ai",
				Secret:     test.secret,
				SecretFile: secretFile,
			})
			defer closeStore()

			if test.wantError != "" {
				if err == nil || !strings.Contains(err.Error(), test.wantError) {
					t.Fatalf("initializeSignedServiceURLs() error = %v, want containing %q", err, test.wantError)
				}
				if service != nil {
					t.Fatal("initializeSignedServiceURLs() service = non-nil with invalid secret file")
				}
				return
			}
			if err != nil {
				t.Fatalf("initializeSignedServiceURLs() error = %v", err)
			}
			if service != nil {
				t.Fatal("initializeSignedServiceURLs() service = non-nil without database configuration")
			}
		})
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
					newExecutorError := test.newExecutorError
					if newExecutorError != nil {
						return nil, newExecutorError

					}
					return stateQueryExecutorStub{}, nil
				},
				newGitHubTrustStore: func(ctx context.Context, _ string) (githubtrust.Store, error) {
					newStoreCalls++
					storeContext = ctx
					newStoreError := test.newStoreError
					if newStoreError != nil {
						return nil, newStoreError

					}
					return githubTrustStoreStub{}, nil
				},
			}
			h := handlers.Handlers{
				Features: handlers.FeaturesWith(stateQueryExecutorStub{}, githubTrustStoreStub{}),
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
			if got := h.Features.StateQuery() != nil; got != test.wantExecutor {
				t.Fatalf("StateQueryExecutor present = %t, want %t", got, test.wantExecutor)
			}
			if got := h.Features.TrustStore() != nil; got != test.wantStore {
				t.Fatalf("GitHubTrustPolicies present = %t, want %t", got, test.wantStore)
			}
			assertStartupContext(t, requireVersionContext, test.wantRequireVersion)
			assertStartupContext(t, storeContext, test.wantNewStore)
		})
	}
}

func TestInitializeDatabaseFeaturesDoesNotLogDatabaseCauses(t *testing.T) {
	const secret = "postgres://user:secret-password@db.internal/cyclops"
	tests := []struct {
		name         string
		config       config.DatabaseConfiguration
		dependencies databaseFeatureDependencies
		wantMessage  string
		wantClass    string
	}{
		{
			name: "state query",
			config: config.DatabaseConfiguration{
				StateQueryDSN:            "postgres://state-query/state-query",
				StateQueryTenantPassword: "tenant-password",
			},
			dependencies: databaseFeatureDependencies{
				requireVersion: func(context.Context, string, int64) error { return nil },
				newStateQueryExecutor: func(string, string) (handlers.StateQueryExecutor, error) {
					return nil, errors.New(secret)
				},
				newGitHubTrustStore: func(context.Context, string) (githubtrust.Store, error) { return nil, nil },
			},
			wantMessage: "kubernetes state query: executor init failed",
			wantClass:   "state_query_initialization_failed",
		},
		{
			name:   "schema",
			config: config.DatabaseConfiguration{URL: "postgres://application/application"},
			dependencies: databaseFeatureDependencies{
				requireVersion:        func(context.Context, string, int64) error { return errors.New(secret) },
				newStateQueryExecutor: func(string, string) (handlers.StateQueryExecutor, error) { return nil, nil },
				newGitHubTrustStore:   func(context.Context, string) (githubtrust.Store, error) { return nil, nil },
			},
			wantMessage: "postgres database schema unavailable",
			wantClass:   "internal",
		},
		{
			name:   "trust store",
			config: config.DatabaseConfiguration{URL: "postgres://application/application"},
			dependencies: databaseFeatureDependencies{
				requireVersion:        func(context.Context, string, int64) error { return nil },
				newStateQueryExecutor: func(string, string) (handlers.StateQueryExecutor, error) { return nil, nil },
				newGitHubTrustStore: func(context.Context, string) (githubtrust.Store, error) {
					return nil, errors.New(secret)
				},
			},
			wantMessage: "github trust policies: init failed",
			wantClass:   "internal",
		},
	}

	for _, testCase := range tests {
		t.Run(testCase.name, func(t *testing.T) {
			var logged bytes.Buffer
			restore := slog.Default()
			slog.SetDefault(slog.New(slog.NewJSONHandler(&logged, nil)))
			t.Cleanup(func() { slog.SetDefault(restore) })

			h := handlers.Handlers{Features: handlers.NewFeatures()}
			initializeDatabaseFeatures(context.Background(), testCase.config, &h, testCase.dependencies)

			if strings.Contains(logged.String(), secret) || strings.Contains(logged.String(), "secret-password") {
				t.Fatalf("database startup log leaked cause:\n%s", logged.String())
			}
			if !strings.Contains(logged.String(), testCase.wantMessage) {
				t.Fatalf("database startup log omitted event message %q:\n%s", testCase.wantMessage, logged.String())
			}
			if !strings.Contains(logged.String(), `"class":"`+testCase.wantClass+`"`) {
				t.Fatalf("database startup log omitted class %q:\n%s", testCase.wantClass, logged.String())
			}
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

// The serving tier keeps readiness database-independent on purpose, so a pod
// that comes up without its database stays Ready and answers 503 from the
// database-backed routes only. cyclops_cs_database_features_ready is what makes
// that state visible; these cases pin the three values it can report.
func TestInitializeDatabaseFeaturesReportsReadinessMetric(t *testing.T) {
	tests := []struct {
		name                string
		config              config.DatabaseConfiguration
		requireVersionError error
		newStoreError       error
		newExecutorError    error
		wantConfigured      string
		wantValue           float64
		wantStateQuery      string
		wantStateQueryValue float64
	}{
		{
			name:                "unset database url reports the disabled configuration as ready",
			config:              config.DatabaseConfiguration{},
			wantConfigured:      "false",
			wantValue:           1,
			wantStateQuery:      "false",
			wantStateQueryValue: 1,
		},
		{
			name: "state query failure is reported independently of the application database",
			config: config.DatabaseConfiguration{
				URL:                      "postgres://application/application",
				StateQueryDSN:            "postgres://state-query/state-query",
				StateQueryTenantPassword: "tenant-password",
			},
			newExecutorError:    fmt.Errorf("invalid state query configuration"),
			wantConfigured:      "true",
			wantValue:           1,
			wantStateQuery:      "true",
			wantStateQueryValue: 0,
		},
		{
			name:                "healthy database reports ready",
			config:              config.DatabaseConfiguration{URL: "postgres://application/application"},
			wantConfigured:      "true",
			wantValue:           1,
			wantStateQuery:      "false",
			wantStateQueryValue: 1,
		},
		{
			name:                "unavailable schema reports degraded",
			config:              config.DatabaseConfiguration{URL: "postgres://application/application"},
			requireVersionError: fmt.Errorf("schema unavailable"),
			wantConfigured:      "true",
			wantValue:           0,
			wantStateQuery:      "false",
			wantStateQueryValue: 1,
		},
		{
			name:                "trust store failure reports degraded",
			config:              config.DatabaseConfiguration{URL: "postgres://application/application"},
			newStoreError:       fmt.Errorf("store unavailable"),
			wantConfigured:      "true",
			wantValue:           0,
			wantStateQuery:      "false",
			wantStateQueryValue: 1,
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			metrics.DatabaseFeaturesReady.Reset()
			metrics.StateQueryReady.Reset()
			dependencies := databaseFeatureDependencies{
				requireVersion: func(context.Context, string, int64) error {
					return test.requireVersionError
				},
				newStateQueryExecutor: func(string, string) (handlers.StateQueryExecutor, error) {
					newExecutorError := test.newExecutorError
					if newExecutorError != nil {
						return nil, newExecutorError

					}
					return stateQueryExecutorStub{}, nil
				},
				newGitHubTrustStore: func(context.Context, string) (githubtrust.Store, error) {
					newStoreError := test.newStoreError
					if newStoreError != nil {
						return nil, newStoreError

					}
					return githubTrustStoreStub{}, nil
				},
			}
			h := handlers.Handlers{Features: handlers.NewFeatures()}
			t.Cleanup(func() { auth.SetGitHubTrustResolver(nil) })

			initializeDatabaseFeatures(context.Background(), test.config, &h, dependencies)

			got := gaugeValue(t, metrics.DatabaseFeaturesReady.WithLabelValues(test.wantConfigured))
			if got != test.wantValue {
				t.Fatalf("cyclops_cs_database_features_ready{configured=%q} = %v, want %v",
					test.wantConfigured, got, test.wantValue)
			}
			if test.wantStateQuery != "" {
				gotStateQuery := gaugeValue(t, metrics.StateQueryReady.WithLabelValues(test.wantStateQuery))
				if gotStateQuery != test.wantStateQueryValue {
					t.Fatalf("cyclops_cs_state_query_ready{configured=%q} = %v, want %v",
						test.wantStateQuery, gotStateQuery, test.wantStateQueryValue)
				}
			}
		})
	}
}

func gaugeValue(t *testing.T, gauge prometheus.Gauge) float64 {
	t.Helper()
	var measurement dto.Metric
	if err := gauge.Write(&measurement); err != nil {
		t.Fatalf("read gauge: %v", err)
	}
	return measurement.GetGauge().GetValue()
}

// The point of the retry: a pod that started during a database outage recovers
// on its own. Before this, initialization ran once and the pod stayed degraded
// until someone restarted it.
func TestRetryDatabaseFeaturesRecoversWithoutARestart(t *testing.T) {
	metrics.DatabaseFeaturesReady.Reset()
	attempts := 0
	dependencies := databaseFeatureDependencies{
		requireVersion: func(context.Context, string, int64) error {
			attempts++
			if attempts < 3 {
				return fmt.Errorf("schema unavailable")
			}
			return nil
		},
		newStateQueryExecutor: func(string, string) (handlers.StateQueryExecutor, error) {
			return stateQueryExecutorStub{}, nil
		},
		newGitHubTrustStore: func(context.Context, string) (githubtrust.Store, error) {
			return githubTrustStoreStub{}, nil
		},
	}
	cfg := config.DatabaseConfiguration{URL: "postgres://application/application"}
	h := handlers.Handlers{Features: handlers.NewFeatures()}
	t.Cleanup(func() { auth.SetGitHubTrustResolver(nil) })

	progress := initializeDatabaseFeatures(context.Background(), cfg, &h, dependencies)
	if progress.ready() {
		t.Fatal("startup reported ready despite the schema being unavailable")
	}
	if got := gaugeValue(t, metrics.DatabaseFeaturesReady.WithLabelValues("true")); got != 0 {
		t.Fatalf("gauge after failed startup = %v, want 0", got)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	retryDatabaseFeatures(ctx, cfg, &h, dependencies, progress, time.Millisecond)

	if h.Features.TrustStore() == nil {
		t.Fatal("trust store still absent after the retry succeeded")
	}
	if got := gaugeValue(t, metrics.DatabaseFeaturesReady.WithLabelValues("true")); got != 1 {
		t.Fatalf("gauge after recovery = %v, want 1 so the alert resolves", got)
	}
}

// A retry driven by a broken application database must not disturb a state
// query executor that came up fine — re-running the whole initialization would
// clear it on every tick.
func TestRetryDatabaseFeaturesLeavesWorkingDependenciesAlone(t *testing.T) {
	installed := stateQueryExecutorStub{}
	executorCalls := 0
	dependencies := databaseFeatureDependencies{
		requireVersion: func(context.Context, string, int64) error {
			return fmt.Errorf("schema unavailable")
		},
		newStateQueryExecutor: func(string, string) (handlers.StateQueryExecutor, error) {
			executorCalls++
			return installed, nil
		},
		newGitHubTrustStore: func(context.Context, string) (githubtrust.Store, error) {
			return githubTrustStoreStub{}, nil
		},
	}
	cfg := config.DatabaseConfiguration{
		URL:                      "postgres://application/application",
		StateQueryDSN:            "postgres://state-query/state-query",
		StateQueryTenantPassword: "tenant-password",
	}
	h := handlers.Handlers{Features: handlers.NewFeatures()}
	t.Cleanup(func() { auth.SetGitHubTrustResolver(nil) })

	progress := initializeDatabaseFeatures(context.Background(), cfg, &h, dependencies)
	for range 3 {
		progress = attemptDatabaseFeatures(context.Background(), cfg, &h, dependencies, progress)
		if h.Features.StateQuery() == nil {
			t.Fatal("a retry cleared the working state query executor")
		}
	}
	if executorCalls != 1 {
		t.Fatalf("state query constructed %d times, want 1 — retries must skip what is already up", executorCalls)
	}
}

// The reason the dependencies live behind Features at all: setupRouter copies
// Handlers by value, so a later install has to be visible through an existing
// copy or the retry is pointless.
func TestFeaturesInstallIsVisibleThroughAnExistingHandlersCopy(t *testing.T) {
	h := handlers.Handlers{Features: handlers.NewFeatures()}
	captured := h // what setupRouter and every route handler hold

	h.Features.SetTrustStore(githubTrustStoreStub{})

	if captured.Features.TrustStore() == nil {
		t.Fatal("install not visible through the captured copy; the retry would update nobody")
	}
}

// The loop must stop once everything is up, or a recovered pod carries a ticker
// and a goroutine for the rest of its life.
func TestRetryDatabaseFeaturesStopsOnceReady(t *testing.T) {
	attempts := 0
	dependencies := databaseFeatureDependencies{
		requireVersion: func(context.Context, string, int64) error {
			attempts++
			if attempts < 2 {
				return fmt.Errorf("schema unavailable")
			}
			return nil
		},
		newStateQueryExecutor: func(string, string) (handlers.StateQueryExecutor, error) {
			return stateQueryExecutorStub{}, nil
		},
		newGitHubTrustStore: func(context.Context, string) (githubtrust.Store, error) {
			return githubTrustStoreStub{}, nil
		},
	}
	cfg := config.DatabaseConfiguration{URL: "postgres://application/application"}
	h := handlers.Handlers{Features: handlers.NewFeatures()}
	t.Cleanup(func() { auth.SetGitHubTrustResolver(nil) })

	progress := initializeDatabaseFeatures(context.Background(), cfg, &h, dependencies)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	retryDatabaseFeatures(ctx, cfg, &h, dependencies, progress, time.Millisecond)

	settled := attempts
	time.Sleep(50 * time.Millisecond) // many ticks' worth at a 1ms interval
	if attempts != settled {
		t.Fatalf("attempts kept climbing after recovery (%d -> %d); the loop did not stop", settled, attempts)
	}
}

func TestInitializeUsageProviderDoesNotGateReadiness(t *testing.T) {
	provider, closeProvider, err := initializeUsageProvider(context.Background(), config.UsageConfiguration{
		DatabaseURL:       "postgres://cyclops_usage_reader:secret@127.0.0.1:1/cyclops?sslmode=disable",
		QueryWebhookURL:   "https://cua-temporal-webhook.tail204509.ts.net/hooks/opencost-query",
		QueryHMACSecret:   "secret",
		QueryResultBucket: "nanoclaw-telemetry-files",
		QueryResultPrefix: "cyclops/usage-query",
		QueryCluster:      "kopf-k3s",
		QueryEnvironment:  "production",
		QueryTimeout:      time.Second,
		QueryPollInterval: time.Second,
		MaxResponseBytes:  64 * 1024,
	})
	if err != nil {
		t.Fatalf("initializeUsageProvider() error = %v", err)
	}
	defer closeProvider()
	if provider == nil {
		t.Fatal("initializeUsageProvider() provider = nil")
	}

	router := setupRouter(handlers.Handlers{Usage: provider})
	response := httptest.NewRecorder()
	router.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/readyz", nil))
	if response.Code != http.StatusOK {
		t.Fatalf("readyz status = %d, want %d", response.Code, http.StatusOK)
	}

	_, err = provider.Overview(context.Background(), usage.Query{ActorSubject: "user", Subject: "user", Timeframe: usage.Timeframe24H})
	if err == nil {
		t.Fatal("Overview() error = nil, want unavailable dependency error")
	}
}

func TestInitializeUsageProviderQueryTransportOnlyIsDisabled(t *testing.T) {
	provider, closeProvider, err := initializeUsageProvider(context.Background(), config.UsageConfiguration{
		QueryWebhookURL:   "https://cua-temporal-webhook.tail204509.ts.net/hooks/opencost-query",
		QueryHMACSecret:   "secret",
		QueryResultBucket: "nanoclaw-telemetry-files",
		QueryResultPrefix: "cyclops/usage-query",
		QueryCluster:      "kopf-k3s",
		QueryEnvironment:  "production",
		QueryTimeout:      time.Second,
		QueryPollInterval: time.Second,
		MaxResponseBytes:  64 * 1024,
	})
	if err != nil {
		t.Fatalf("initializeUsageProvider() error = %v", err)
	}
	defer closeProvider()
	if provider != nil {
		t.Fatal("initializeUsageProvider() provider != nil, want disabled provider")
	}
}

func TestProductAnalyticsClientConfigPreservesRuntimeSettings(t *testing.T) {
	cfg := productAnalyticsClientConfig(config.ProductAnalyticsConfiguration{
		Enabled: true, Host: "https://eu.i.posthog.com", ProjectToken: "phc_test",
		Environment: "production", ExcludedSubjects: []string{"internal-1"},
	})
	want := productanalytics.Config{
		Enabled: true, Host: "https://eu.i.posthog.com", ProjectToken: "phc_test",
		Environment: "production", ExcludedSubjects: []string{"internal-1"},
	}
	if !reflect.DeepEqual(cfg, want) {
		t.Fatalf("config = %#v, want %#v", cfg, want)
	}
}

func TestServeUntilCanceledShutsDownServer(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	server := &http.Server{Addr: "127.0.0.1:0", Handler: http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) })}
	go func() {
		time.Sleep(20 * time.Millisecond)
		cancel()
	}()
	if err := serveUntilCanceled(ctx, server); err != nil {
		t.Fatalf("serveUntilCanceled() error = %v", err)
	}
}

func TestShutdownSignedServiceURLsCancelsAndJoinsBeforeClosingOnce(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	var retry sync.WaitGroup
	var mu sync.RWMutex
	retryStarted := make(chan struct{})
	retryStopped := make(chan struct{})
	retry.Add(1)
	go func() {
		defer retry.Done()
		close(retryStarted)
		<-ctx.Done()
		close(retryStopped)
	}()
	<-retryStarted
	closeCalls := 0
	closeStore := func() {
		select {
		case <-retryStopped:
		default:
			t.Fatal("store closed before retry goroutine stopped")
		}
		closeCalls++
	}

	shutdownSignedServiceURLs(cancel, &retry, &mu, &closeStore)
	shutdownSignedServiceURLs(cancel, &retry, &mu, &closeStore)

	if closeCalls != 1 {
		t.Fatalf("close calls = %d, want 1", closeCalls)
	}
}
