package handlers

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/config"
	"cyclops-cs-backend/signedurls"

	"github.com/google/uuid"
)

const capabilityToken = "sensitive-capability-token"

type fakeSignedServiceURLService struct {
	createCalls     int
	createInput     signedurls.CreateInput
	createRecord    signedurls.Record
	createErr       error
	listNamespace   string
	listClaim       string
	listRecords     []signedurls.Record
	listErr         error
	revokeNamespace string
	revokeID        uuid.UUID
	revokeRecord    signedurls.Record
	revokeCalls     int
	revokeErr       error
}

func (service *fakeSignedServiceURLService) Create(_ context.Context, input signedurls.CreateInput) (signedurls.Record, error) {
	service.createCalls++
	service.createInput = input
	return service.createRecord, service.createErr
}

func (service *fakeSignedServiceURLService) List(_ context.Context, namespace, claim string) ([]signedurls.Record, error) {
	service.listNamespace, service.listClaim = namespace, claim
	return service.listRecords, service.listErr
}

func (service *fakeSignedServiceURLService) Revoke(_ context.Context, namespace string, id uuid.UUID) (signedurls.Record, error) {
	service.revokeNamespace, service.revokeID = namespace, id
	service.revokeCalls++
	return service.revokeRecord, service.revokeErr
}

func withTestUser(ctx context.Context, user *auth.User) context.Context {
	return context.WithValue(ctx, auth.UserKey, user)
}

func TestCreateSignedServiceURLPassesCallerSubject(t *testing.T) {
	service := &fakeSignedServiceURLService{createRecord: signedServiceURLRecord()}
	handlers := Handlers{signedServiceURLs: service, signedServiceExists: func(context.Context, string, string, string) (bool, error) { return true, nil }}
	request := httptest.NewRequest(http.MethodPost, "/api/signed-service-urls/ns-a", strings.NewReader(`{
  "claim":"claim-a","sandbox":"sandbox-a","service":"sandbox-a-mcp",
  "logicalService":"mcp","label":"Customer demo","expiresInSeconds":3600
}`))
	request.SetPathValue("namespace", "ns-a")
	request = request.WithContext(withTestUser(request.Context(), &auth.User{ID: "user-a"}))
	response := httptest.NewRecorder()

	handlers.CreateSignedServiceURL(response, request)

	if response.Code != http.StatusCreated {
		t.Fatalf("status = %d, want 201; body = %s", response.Code, response.Body.String())
	}
	if service.createInput.CreatorSub != "user-a" {
		t.Fatalf("creator subject = %q, want user-a", service.createInput.CreatorSub)
	}
	if got, want := service.createInput.ExpiresIn, time.Hour; got != want {
		t.Fatalf("expires in = %s, want %s", got, want)
	}
	if got, want := service.createInput.ServiceName, "sandbox-a-mcp"; got != want {
		t.Fatalf("service name = %q, want derived %q", got, want)
	}
	var body SignedServiceURLResponse
	if err := json.NewDecoder(response.Body).Decode(&body); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	if body.URL != signedServiceURLRecord().URL {
		t.Fatalf("URL = %q, want %q", body.URL, signedServiceURLRecord().URL)
	}
}

func TestCreateSignedServiceURLValidatesAndBindsPhysicalService(t *testing.T) {
	for name, body := range map[string]string{
		"invalid namespace":  `{"claim":"claim-a","sandbox":"sandbox-a","service":"sandbox-a-mcp","logicalService":"mcp","expiresInSeconds":3600}`,
		"invalid claim":      `{"claim":"claim/a","sandbox":"sandbox-a","service":"sandbox-a-mcp","logicalService":"mcp","expiresInSeconds":3600}`,
		"invalid sandbox":    `{"claim":"claim-a","sandbox":"Sandbox-A","service":"Sandbox-A-mcp","logicalService":"mcp","expiresInSeconds":3600}`,
		"invalid logical":    `{"claim":"claim-a","sandbox":"sandbox-a","service":"sandbox-a-../mcp","logicalService":"../mcp","expiresInSeconds":3600}`,
		"mismatched service": `{"claim":"claim-a","sandbox":"sandbox-a","service":"other-service","logicalService":"mcp","expiresInSeconds":3600}`,
	} {
		t.Run(name, func(t *testing.T) {
			service := &fakeSignedServiceURLService{createRecord: signedServiceURLRecord()}
			handler := Handlers{signedServiceURLs: service, signedServiceExists: func(context.Context, string, string, string) (bool, error) { return true, nil }}
			request := signedServiceURLRequest(http.MethodPost, "ns-a", "", &auth.User{ID: "user-a"})
			if name == "invalid namespace" {
				request.SetPathValue("namespace", "ns/../other")
			}
			request.Body = io.NopCloser(strings.NewReader(body))
			response := httptest.NewRecorder()
			handler.CreateSignedServiceURL(response, request)
			if response.Code != http.StatusBadRequest {
				t.Fatalf("status = %d, want 400; body = %s", response.Code, response.Body.String())
			}
			if service.createCalls != 0 {
				t.Fatalf("Create calls = %d, want 0", service.createCalls)
			}
		})
	}
}

func TestCreateSignedServiceURLChecksKubernetesServiceBeforePersisting(t *testing.T) {
	fakeK8s := newFakeK8s(http.StatusNotFound, `{"kind":"Status"}`)
	t.Cleanup(fakeK8s.server.Close)
	overrideK8sClient(fakeK8s.server.Client(), fakeK8s.server.URL, "fake-sa-token")
	service := &fakeSignedServiceURLService{createRecord: signedServiceURLRecord()}
	handler := Handlers{signedServiceURLs: service, checkSignedServiceExists: true}
	request := signedServiceURLRequest(http.MethodPost, "ns-a", "", &auth.User{ID: "user-a"})
	request.Body = io.NopCloser(strings.NewReader(signedServiceURLBody))
	response := httptest.NewRecorder()
	handler.CreateSignedServiceURL(response, request)
	if response.Code != http.StatusNotFound {
		t.Fatalf("status = %d, want 404; body = %s", response.Code, response.Body.String())
	}
	if service.createCalls != 0 {
		t.Fatalf("Create calls = %d, want 0", service.createCalls)
	}
	if len(fakeK8s.requests) != 1 || fakeK8s.requests[0].method != http.MethodGet || fakeK8s.requests[0].path != "/api/v1/namespaces/ns-a/services/sandbox-a-mcp" {
		t.Fatalf("Kubernetes requests = %#v, want GET service existence check", fakeK8s.requests)
	}
}

func TestCreateSignedServiceURLReturnsUnavailableWhenKubernetesCannotValidate(t *testing.T) {
	fakeK8s := newFakeK8s(http.StatusInternalServerError, "boom")
	t.Cleanup(fakeK8s.server.Close)
	overrideK8sClient(fakeK8s.server.Client(), fakeK8s.server.URL, "fake-sa-token")
	handler := Handlers{signedServiceURLs: &fakeSignedServiceURLService{createRecord: signedServiceURLRecord()}, checkSignedServiceExists: true}
	request := signedServiceURLRequest(http.MethodPost, "ns-a", "", &auth.User{ID: "user-a"})
	request.Body = io.NopCloser(strings.NewReader(signedServiceURLBody))
	response := httptest.NewRecorder()
	handler.CreateSignedServiceURL(response, request)
	if response.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, want 503; body = %s", response.Code, response.Body.String())
	}
}

func TestListSignedServiceURLsScopesToCallerAndNamespace(t *testing.T) {
	service := &fakeSignedServiceURLService{listRecords: []signedurls.Record{signedServiceURLRecord(), secondSignedServiceURLRecord()}}
	request := httptest.NewRequest(http.MethodGet, "/api/signed-service-urls/ns-a?claim=claim-a", nil)
	request.SetPathValue("namespace", "ns-a")
	request = request.WithContext(withTestUser(request.Context(), &auth.User{ID: "user-a"}))
	response := httptest.NewRecorder()

	Handlers{signedServiceURLs: service}.ListSignedServiceURLs(response, request)

	if response.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200; body = %s", response.Code, response.Body.String())
	}
	if service.listNamespace != "ns-a" || service.listClaim != "claim-a" {
		t.Fatalf("list scope = (%q, %q), want (ns-a, claim-a)", service.listNamespace, service.listClaim)
	}
	var body []SignedServiceURLResponse
	if err := json.NewDecoder(response.Body).Decode(&body); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	if len(body) != 2 || body[0].Service != "sandbox-a-mcp" || body[1].Service != "sandbox-a-mcp" || body[0].ID == body[1].ID || body[0].Label == nil || body[1].Label == nil || *body[0].Label == *body[1].Label {
		t.Fatalf("response = %+v, want two distinct records for one service", body)
	}
}

func TestListSignedServiceURLsRequiresValidClaim(t *testing.T) {
	for _, target := range []string{"/api/signed-service-urls/ns-a", "/api/signed-service-urls/ns-a?claim=invalid/claim"} {
		request := httptest.NewRequest(http.MethodGet, target, nil)
		request.SetPathValue("namespace", "ns-a")
		request = request.WithContext(withTestUser(request.Context(), &auth.User{ID: "user-a"}))
		response := httptest.NewRecorder()

		Handlers{signedServiceURLs: &fakeSignedServiceURLService{}}.ListSignedServiceURLs(response, request)

		if response.Code != http.StatusBadRequest {
			t.Fatalf("target %q status = %d, want 400", target, response.Code)
		}
	}
}

func TestRevokeSignedServiceURLIsIdempotent(t *testing.T) {
	record := signedServiceURLRecord()
	service := &fakeSignedServiceURLService{revokeRecord: record}
	request := signedServiceURLRequest(http.MethodDelete, "ns-a", record.ID.String(), &auth.User{ID: "user-a"})
	for attempt := 1; attempt <= 2; attempt++ {
		response := httptest.NewRecorder()
		Handlers{signedServiceURLs: service}.RevokeSignedServiceURL(response, request)
		if response.Code != http.StatusNoContent {
			t.Fatalf("attempt %d status = %d, want 204; body = %s", attempt, response.Code, response.Body.String())
		}
	}
	if service.revokeCalls != 2 || service.revokeNamespace != "ns-a" || service.revokeID != record.ID {
		t.Fatalf("revoke scope = (%q, %s)", service.revokeNamespace, service.revokeID)
	}
}

func TestSignedServiceURLStoreFailuresReturnInternalServerError(t *testing.T) {
	cases := map[string]struct {
		service *fakeSignedServiceURLService
		handle  func(Handlers, http.ResponseWriter, *http.Request)
		request func() *http.Request
	}{
		"create": {
			service: &fakeSignedServiceURLService{createErr: errors.New("store failed")},
			handle:  func(h Handlers, w http.ResponseWriter, r *http.Request) { h.CreateSignedServiceURL(w, r) },
			request: func() *http.Request {
				request := signedServiceURLRequest(http.MethodPost, "ns-a", "", &auth.User{ID: "user-a"})
				request.Body = io.NopCloser(strings.NewReader(signedServiceURLBody))
				return request
			},
		},
		"list": {
			service: &fakeSignedServiceURLService{listErr: errors.New("store failed")},
			handle:  func(h Handlers, w http.ResponseWriter, r *http.Request) { h.ListSignedServiceURLs(w, r) },
			request: func() *http.Request {
				request := signedServiceURLRequest(http.MethodGet, "ns-a", "", &auth.User{ID: "user-a"})
				query := request.URL.Query()
				query.Set("claim", "claim-a")
				request.URL.RawQuery = query.Encode()
				return request
			},
		},
		"revoke": {
			service: &fakeSignedServiceURLService{revokeErr: errors.New("store failed")},
			handle:  func(h Handlers, w http.ResponseWriter, r *http.Request) { h.RevokeSignedServiceURL(w, r) },
			request: func() *http.Request {
				return signedServiceURLRequest(http.MethodDelete, "ns-a", signedServiceURLRecord().ID.String(), &auth.User{ID: "user-a"})
			},
		},
	}

	for name, testCase := range cases {
		t.Run(name, func(t *testing.T) {
			response := httptest.NewRecorder()
			testCase.handle(Handlers{signedServiceURLs: testCase.service}, response, testCase.request())
			if response.Code != http.StatusInternalServerError {
				t.Fatalf("status = %d, want %d", response.Code, http.StatusInternalServerError)
			}
		})
	}
}

func TestSignedServiceURLHandlerErrors(t *testing.T) {
	var logs bytes.Buffer
	previousLogger := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(&logs, nil)))
	t.Cleanup(func() { slog.SetDefault(previousLogger) })
	for name, testCase := range map[string]struct {
		method  string
		body    string
		id      string
		user    *auth.User
		service *fakeSignedServiceURLService
		want    int
	}{
		"malformed UUID":      {method: http.MethodDelete, id: "not-a-uuid", user: &auth.User{ID: "user-a"}, service: &fakeSignedServiceURLService{}, want: http.StatusBadRequest},
		"invalid body":        {method: http.MethodPost, body: `{`, user: &auth.User{ID: "user-a"}, service: &fakeSignedServiceURLService{}, want: http.StatusBadRequest},
		"missing user":        {method: http.MethodPost, body: signedServiceURLBody, service: &fakeSignedServiceURLService{}, want: http.StatusUnauthorized},
		"unavailable feature": {method: http.MethodPost, body: signedServiceURLBody, user: &auth.User{ID: "user-a"}, service: nil, want: http.StatusServiceUnavailable},
		"invalid TTL":         {method: http.MethodPost, body: signedServiceURLBody, user: &auth.User{ID: "user-a"}, service: &fakeSignedServiceURLService{createErr: signedurls.ErrInvalidInput}, want: http.StatusBadRequest},
		"store failure":       {method: http.MethodPost, body: signedServiceURLBody, user: &auth.User{ID: "user-a"}, service: &fakeSignedServiceURLService{createErr: errors.New("store failed " + capabilityToken)}, want: http.StatusInternalServerError},
		"store unavailable":   {method: http.MethodPost, body: signedServiceURLBody, user: &auth.User{ID: "user-a"}, service: &fakeSignedServiceURLService{createErr: signedurls.ErrUnavailable}, want: http.StatusServiceUnavailable},
		"missing record":      {method: http.MethodDelete, id: signedServiceURLRecord().ID.String(), user: &auth.User{ID: "user-a"}, service: &fakeSignedServiceURLService{revokeErr: signedurls.ErrNotFound}, want: http.StatusNotFound},
	} {
		t.Run(name, func(t *testing.T) {
			request := signedServiceURLRequest(testCase.method, "ns-a", testCase.id, testCase.user)
			if testCase.body != "" {
				request.Body = io.NopCloser(strings.NewReader(testCase.body))
			}
			response := httptest.NewRecorder()
			handlers := Handlers{signedServiceURLs: testCase.service, signedServiceExists: func(context.Context, string, string, string) (bool, error) { return true, nil }}
			if testCase.service == nil {
				handlers = Handlers{}
			}
			if testCase.method == http.MethodPost {
				handlers.CreateSignedServiceURL(response, request)
			} else {
				handlers.RevokeSignedServiceURL(response, request)
			}
			if response.Code != testCase.want {
				t.Fatalf("status = %d, want %d; body = %s", response.Code, testCase.want, response.Body.String())
			}
			if strings.Contains(response.Body.String(), capabilityToken) {
				t.Fatalf("response leaked capability token: %s", response.Body.String())
			}
			if strings.Contains(logs.String(), capabilityToken) {
				t.Fatalf("logs leaked capability token: %s", logs.String())
			}
		})
	}
}

const signedServiceURLBody = `{"claim":"claim-a","sandbox":"sandbox-a","service":"sandbox-a-mcp","logicalService":"mcp","expiresInSeconds":3600}`

func signedServiceURLRequest(method, namespace, id string, user *auth.User) *http.Request {
	request := httptest.NewRequest(method, "/api/signed-service-urls/"+namespace, nil)
	request.SetPathValue("namespace", namespace)
	if id != "" {
		request.SetPathValue("id", id)
	}
	if user != nil {
		request = request.WithContext(withTestUser(request.Context(), user))
	}
	return request
}

func signedServiceURLRecord() signedurls.Record {
	label := "Customer demo"
	return signedurls.Record{ID: uuid.MustParse("5cd7f3e4-5390-4c0c-a93b-dd18116d367c"), Namespace: "ns-a", ClaimName: "claim-a", SandboxName: "sandbox-a", ServiceName: "sandbox-a-mcp", LogicalService: "mcp", Label: &label, CreatedAt: time.Date(2026, time.August, 31, 10, 0, 0, 0, time.UTC), ExpiresAt: time.Date(2026, time.August, 31, 11, 0, 0, 0, time.UTC), URL: "https://api.example.test/api/signed-svc/" + capabilityToken}
}

func secondSignedServiceURLRecord() signedurls.Record {
	record := signedServiceURLRecord()
	record.ID = uuid.MustParse("2a8cc1f2-33c9-40e1-8de6-8de4c615ef9a")
	record.ServiceName = "sandbox-a-mcp"
	label := "Support session"
	record.Label = &label
	return record
}

func TestSignedServiceURLValidationSubject(t *testing.T) {
	for _, testCase := range []struct {
		name         string
		user         *auth.User
		keyClientPfx string
		want         string
	}{
		{name: "normal user", user: &auth.User{ID: "user-a", AZP: "cyclops-cs-spa"}, want: "user-a"},
		{name: "user key", user: &auth.User{ID: "user-a", AZP: "ukey-user-a"}, want: "user-a"},
		{name: "GitHub OIDC", user: &auth.User{ID: "user-a", PrincipalType: auth.PrincipalTypeGitHubOIDC}, want: "user-a"},
		{name: "per-key namespace claim", user: &auth.User{ID: "service-account", AZP: "key-ns-a", Namespace: "ns-a"}, want: ""},
		{name: "per-key missing namespace claim", user: &auth.User{ID: "service-account", AZP: "key-ns-a"}, want: "service-account"},
		{name: "custom per-key namespace claim", user: &auth.User{ID: "service-account", AZP: "poolkey-ns-a", Namespace: "ns-a"}, keyClientPfx: "poolkey-", want: ""},
		{name: "legacy per-key prefix under custom configuration", user: &auth.User{ID: "service-account", AZP: "key-ns-a", Namespace: "ns-a"}, keyClientPfx: "poolkey-", want: "service-account"},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			if got := signedServiceURLValidationSubject(testCase.user, testCase.keyClientPfx); got != testCase.want {
				t.Fatalf("subject = %q, want %q", got, testCase.want)
			}
		})
	}
}

func TestCreateSignedServiceURLAuthorizesClaimTemplateAndService(t *testing.T) {
	fakeK8s := newFakeK8sSequence(
		fakeK8sResponse{status: http.StatusOK, body: `{"metadata":{"name":"claim-a"},"spec":{"sandboxTemplateRef":{"name":"template-a"}},"status":{"phase":"Bound","sandbox":{"name":"sandbox-a"}}}`},
		fakeK8sResponse{status: http.StatusOK, body: `{"spec":{"vmTemplate":{"services":[{"name":"mcp"},{"name":"vnc"}]}}}`},
		fakeK8sResponse{status: http.StatusOK, body: `{"metadata":{"name":"sandbox-a-mcp"}}`},
	)
	t.Cleanup(fakeK8s.server.Close)
	overrideK8sClient(fakeK8s.server.Client(), fakeK8s.server.URL, "fake-sa-token")
	service := &fakeSignedServiceURLService{createRecord: signedServiceURLRecord()}
	handler := Handlers{
		signedServiceURLs:        service,
		SignedServiceURLProvider: func() *signedurls.Service { return nil },
		checkSignedServiceExists: true,
	}
	request := signedServiceURLRequest(http.MethodPost, "ns-a", "", &auth.User{ID: "user-a"})
	request.Body = io.NopCloser(strings.NewReader(signedServiceURLBody))
	response := httptest.NewRecorder()

	handler.CreateSignedServiceURL(response, request)

	if response.Code != http.StatusCreated {
		t.Fatalf("status = %d, want 201; body = %s", response.Code, response.Body.String())
	}
	wantPaths := []string{
		"/apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims/claim-a",
		"/apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxtemplates/template-a",
		"/api/v1/namespaces/ns-a/services/sandbox-a-mcp",
	}
	if len(fakeK8s.requests) != len(wantPaths) {
		t.Fatalf("Kubernetes request count = %d, want %d: %#v", len(fakeK8s.requests), len(wantPaths), fakeK8s.requests)
	}
	for index, wantPath := range wantPaths {
		got := fakeK8s.requests[index]
		if got.method != http.MethodGet || got.path != wantPath {
			t.Fatalf("request %d = %s %s, want GET %s", index, got.method, got.path, wantPath)
		}
		if got.headers.Get("Impersonate-User") != "oidc:user-a" || got.headers.Get("Impersonate-Group") == "" {
			t.Fatalf("request %d impersonation headers = %#v", index, got.headers)
		}
	}
	if service.createCalls != 1 || service.createInput.ServiceName != "sandbox-a-mcp" {
		t.Fatalf("create calls/input = %d/%+v", service.createCalls, service.createInput)
	}
}

func TestCreateSignedServiceURLPerKeyUsesBackendServiceAccountForValidation(t *testing.T) {
	fakeK8s := newFakeK8sSequence(
		fakeK8sResponse{status: http.StatusOK, body: `{"metadata":{"name":"claim-a"},"spec":{"sandboxTemplateRef":{"name":"template-a"}},"status":{"phase":"Bound","sandbox":{"name":"sandbox-a"}}}`},
		fakeK8sResponse{status: http.StatusOK, body: `{"spec":{"vmTemplate":{"services":[{"name":"mcp"}]}}}`},
		fakeK8sResponse{status: http.StatusOK, body: `{"metadata":{"name":"sandbox-a-mcp"}}`},
	)
	t.Cleanup(fakeK8s.server.Close)
	overrideK8sClient(fakeK8s.server.Client(), fakeK8s.server.URL, "fake-sa-token")
	service := &fakeSignedServiceURLService{createRecord: signedServiceURLRecord()}
	handler := Handlers{
		signedServiceURLs:        service,
		SignedServiceURLProvider: func() *signedurls.Service { return nil },
		checkSignedServiceExists: true,
	}
	request := signedServiceURLRequest(http.MethodPost, "ns-a", "", &auth.User{ID: "service-account", AZP: "key-ns-a", Namespace: "ns-a"})
	request.Body = io.NopCloser(strings.NewReader(signedServiceURLBody))
	response := httptest.NewRecorder()

	handler.CreateSignedServiceURL(response, request)

	if response.Code != http.StatusCreated {
		t.Fatalf("status = %d, want 201; body = %s", response.Code, response.Body.String())
	}
	if len(fakeK8s.requests) != 3 {
		t.Fatalf("Kubernetes request count = %d, want 3: %#v", len(fakeK8s.requests), fakeK8s.requests)
	}
	for index, request := range fakeK8s.requests {
		if request.headers.Get("Authorization") != "Bearer fake-sa-token" {
			t.Fatalf("request %d authorization = %q, want backend ServiceAccount token", index, request.headers.Get("Authorization"))
		}
		if request.headers.Get("Impersonate-User") != "" || request.headers.Get("Impersonate-Group") != "" {
			t.Fatalf("request %d must not impersonate per-key caller: %#v", index, request.headers)
		}
	}
	if service.createCalls != 1 || service.createInput.CreatorSub != "service-account" {
		t.Fatalf("create calls/input = %d/%+v", service.createCalls, service.createInput)
	}
}

func TestCreateSignedServiceURLCustomPerKeyPrefixUsesBackendServiceAccount(t *testing.T) {
	fakeK8s := newFakeK8sSequence(
		fakeK8sResponse{status: http.StatusOK, body: `{"metadata":{"name":"claim-a"},"spec":{"sandboxTemplateRef":{"name":"template-a"}},"status":{"phase":"Bound","sandbox":{"name":"sandbox-a"}}}`},
		fakeK8sResponse{status: http.StatusOK, body: `{"spec":{"vmTemplate":{"services":[{"name":"mcp"}]}}}`},
		fakeK8sResponse{status: http.StatusOK, body: `{"metadata":{"name":"sandbox-a-mcp"}}`},
	)
	t.Cleanup(fakeK8s.server.Close)
	overrideK8sClient(fakeK8s.server.Client(), fakeK8s.server.URL, "fake-sa-token")
	service := &fakeSignedServiceURLService{createRecord: signedServiceURLRecord()}
	handler := Handlers{
		AuthCfg:                  config.AuthConfiguration{KeyClientPfx: "poolkey-"},
		signedServiceURLs:        service,
		SignedServiceURLProvider: func() *signedurls.Service { return nil },
		checkSignedServiceExists: true,
	}
	request := signedServiceURLRequest(http.MethodPost, "ns-a", "", &auth.User{ID: "service-account", AZP: "poolkey-ns-a", Namespace: "ns-a"})
	request.Body = io.NopCloser(strings.NewReader(signedServiceURLBody))
	response := httptest.NewRecorder()

	handler.CreateSignedServiceURL(response, request)

	if response.Code != http.StatusCreated {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusCreated, response.Body.String())
	}
	if len(fakeK8s.requests) != 3 {
		t.Fatalf("Kubernetes request count = %d, want 3: %#v", len(fakeK8s.requests), fakeK8s.requests)
	}
	for index, request := range fakeK8s.requests {
		if request.headers.Get("Authorization") != "Bearer fake-sa-token" {
			t.Fatalf("request %d authorization = %q, want backend ServiceAccount token", index, request.headers.Get("Authorization"))
		}
		if request.headers.Get("Impersonate-User") != "" || request.headers.Get("Impersonate-Group") != "" {
			t.Fatalf("request %d must not impersonate configured per-key caller: %#v", index, request.headers)
		}
	}
}

func TestCreateSignedServiceURLLegacyPerKeyPrefixUsesCallerIdentityWithCustomConfiguration(t *testing.T) {
	fakeK8s := newFakeK8sSequence(
		fakeK8sResponse{status: http.StatusOK, body: `{"metadata":{"name":"claim-a"},"spec":{"sandboxTemplateRef":{"name":"template-a"}},"status":{"phase":"Bound","sandbox":{"name":"sandbox-a"}}}`},
		fakeK8sResponse{status: http.StatusOK, body: `{"spec":{"vmTemplate":{"services":[{"name":"mcp"}]}}}`},
		fakeK8sResponse{status: http.StatusOK, body: `{"metadata":{"name":"sandbox-a-mcp"}}`},
	)
	t.Cleanup(fakeK8s.server.Close)
	overrideK8sClient(fakeK8s.server.Client(), fakeK8s.server.URL, "fake-sa-token")
	service := &fakeSignedServiceURLService{createRecord: signedServiceURLRecord()}
	handler := Handlers{
		AuthCfg:                  config.AuthConfiguration{KeyClientPfx: "poolkey-"},
		signedServiceURLs:        service,
		SignedServiceURLProvider: func() *signedurls.Service { return nil },
		checkSignedServiceExists: true,
	}
	request := signedServiceURLRequest(http.MethodPost, "ns-a", "", &auth.User{ID: "service-account", AZP: "key-ns-a", Namespace: "ns-a"})
	request.Body = io.NopCloser(strings.NewReader(signedServiceURLBody))
	response := httptest.NewRecorder()

	handler.CreateSignedServiceURL(response, request)

	if response.Code != http.StatusCreated {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusCreated, response.Body.String())
	}
	if len(fakeK8s.requests) != 3 {
		t.Fatalf("Kubernetes request count = %d, want 3: %#v", len(fakeK8s.requests), fakeK8s.requests)
	}
	for index, request := range fakeK8s.requests {
		if request.headers.Get("Impersonate-User") != "oidc:service-account" || request.headers.Get("Impersonate-Group") == "" {
			t.Fatalf("request %d impersonation headers = %#v, want caller identity", index, request.headers)
		}
	}
}

func TestCreateSignedServiceURLRejectsUnauthorizedClaimRelationships(t *testing.T) {
	boundClaim := `{"metadata":{"name":"claim-a"},"spec":{"sandboxTemplateRef":{"name":"template-a"}},"status":{"phase":"Bound","sandbox":{"name":"sandbox-a"}}}`
	for _, testCase := range []struct {
		name      string
		responses []fakeK8sResponse
	}{
		{name: "missing claim", responses: []fakeK8sResponse{{status: http.StatusNotFound, body: `{"kind":"Status"}`}}},
		{name: "claim not bound", responses: []fakeK8sResponse{{status: http.StatusOK, body: `{"metadata":{"name":"claim-a"},"spec":{"sandboxTemplateRef":{"name":"template-a"}},"status":{"phase":"Pending","sandbox":{"name":"sandbox-a"}}}`}}},
		{name: "claim bound to another sandbox", responses: []fakeK8sResponse{{status: http.StatusOK, body: `{"metadata":{"name":"claim-a"},"spec":{"sandboxTemplateRef":{"name":"template-a"}},"status":{"phase":"Bound","sandbox":{"name":"sandbox-b"}}}`}}},
		{name: "logical service absent from template", responses: []fakeK8sResponse{{status: http.StatusOK, body: boundClaim}, {status: http.StatusOK, body: `{"spec":{"vmTemplate":{"services":[{"name":"vnc"}]}}}`}}},
		{name: "top-level services do not authorize", responses: []fakeK8sResponse{{status: http.StatusOK, body: boundClaim}, {status: http.StatusOK, body: `{"spec":{"services":[{"name":"mcp"}]}}`}}},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			fakeK8s := newFakeK8sSequence(testCase.responses...)
			t.Cleanup(fakeK8s.server.Close)
			overrideK8sClient(fakeK8s.server.Client(), fakeK8s.server.URL, "fake-sa-token")
			service := &fakeSignedServiceURLService{createRecord: signedServiceURLRecord()}
			handler := Handlers{
				signedServiceURLs:        service,
				SignedServiceURLProvider: func() *signedurls.Service { return nil },
				checkSignedServiceExists: true,
			}
			request := signedServiceURLRequest(http.MethodPost, "ns-a", "", &auth.User{ID: "user-a"})
			request.Body = io.NopCloser(strings.NewReader(signedServiceURLBody))
			response := httptest.NewRecorder()

			handler.CreateSignedServiceURL(response, request)

			if response.Code != http.StatusNotFound {
				t.Fatalf("status = %d, want 404; body = %s", response.Code, response.Body.String())
			}
			if service.createCalls != 0 {
				t.Fatalf("Create calls = %d, want 0", service.createCalls)
			}
		})
	}
}
