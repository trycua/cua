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
	"cyclops-cs-backend/handlers"

	jwt "github.com/golang-jwt/jwt/v5"
	"github.com/trycua/cloud/pkg/featureflags"
)

// TestDeprecatedRoutes asserts that every deprecated batch/label endpoint
// returns HTTP 410 Gone with the canonical deprecation message.  Each route
// is exercised individually so a regression on any single path is immediately
// visible rather than hidden behind a representative sample.
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

func TestDeprecatedRoutes(t *testing.T) {
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

	const wantMsg = "/api/batch and /api/label are deprecated and unavailable"

	for _, tc := range cases {
		t.Run(tc.method+" "+tc.path, func(t *testing.T) {
			req := authorizedRequest(t, tc.method, tc.path, tc.body)
			w := httptest.NewRecorder()
			router.ServeHTTP(w, req)

			if w.Code != http.StatusGone {
				t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusGone, w.Body.String())
			}
			if !strings.Contains(w.Body.String(), wantMsg) {
				t.Fatalf("body = %q, want deprecation message %q", w.Body.String(), wantMsg)
			}
		})
	}
}

func TestSwaggerMentionsBatchRouteDeprecation(t *testing.T) {
	data, err := os.ReadFile("docs/swagger.json")
	if err != nil {
		t.Fatal(err)
	}
	body := string(data)
	if !strings.Contains(body, "\"410\"") {
		t.Fatalf("swagger.json missing 410 response for deprecated batch routes")
	}
	if !strings.Contains(body, "deprecated and unavailable") {
		t.Fatalf("swagger.json missing deprecation language")
	}

	var spec struct {
		Paths map[string]map[string]struct {
			Description string         `json:"description"`
			Deprecated  *bool          `json:"deprecated"`
			Responses   map[string]any `json:"responses"`
		} `json:"paths"`
	}
	if err := json.Unmarshal(data, &spec); err != nil {
		t.Fatalf("unmarshal swagger.json: %v", err)
	}

	cases := []struct {
		path   string
		method string
	}{
		{path: "/api/batch/{pool}/submit", method: "post"},
		{path: "/api/batch/{pool}/lanes", method: "post"},
		{path: "/api/batch/{pool}/lanes", method: "delete"},
		{path: "/api/batch/{pool}/{id}/status", method: "get"},
		{path: "/api/batch/{pool}/{id}/results", method: "get"},
		{path: "/api/batch/{pool}/{id}", method: "delete"},
		{path: "/api/label/{pool}/{label}/batch", method: "post"},
		{path: "/api/label/{pool}/{label}/status", method: "get"},
		{path: "/api/label/{pool}/{label}/results", method: "get"},
		{path: "/api/label/{pool}/{label}", method: "delete"},
	}
	for _, tc := range cases {
		ops, ok := spec.Paths[tc.path]
		if !ok {
			t.Fatalf("swagger.json missing path %s", tc.path)
		}
		op, ok := ops[tc.method]
		if !ok {
			t.Fatalf("swagger.json missing %s %s", strings.ToUpper(tc.method), tc.path)
		}
		if !strings.Contains(op.Description, "deprecated and unavailable") {
			t.Fatalf("%s %s description = %q, want deprecation language", strings.ToUpper(tc.method), tc.path, op.Description)
		}
		if op.Deprecated == nil || !*op.Deprecated {
			t.Fatalf("%s %s deprecated = %v, want true", strings.ToUpper(tc.method), tc.path, op.Deprecated)
		}
		if _, ok := op.Responses["410"]; !ok {
			t.Fatalf("%s %s missing 410 response", strings.ToUpper(tc.method), tc.path)
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
