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
	"cyclops-cs-backend/featureflagadmin"
	"cyclops-cs-backend/middlewares"
	"github.com/trycua/cloud/pkg/featureflags"
)

type featureFlagStore struct {
	parameters map[string]featureflags.Parameter
	updatePath string
	deletePath string
	updateErr  error
	deleteErr  error
}

func newFeatureFlagStore(parameters ...featureflags.Parameter) *featureFlagStore {
	store := &featureFlagStore{parameters: map[string]featureflags.Parameter{}}
	for _, parameter := range parameters {
		store.parameters[parameter.Name] = parameter
	}
	return store
}
func (s *featureFlagStore) List(_ context.Context, prefix string) ([]featureflags.Parameter, error) {
	var result []featureflags.Parameter
	for name, parameter := range s.parameters {
		if len(name) >= len(prefix) && name[:len(prefix)] == prefix {
			result = append(result, parameter)
		}
	}
	return result, nil
}
func (s *featureFlagStore) Get(_ context.Context, path string) (featureflags.Parameter, error) {
	parameter, ok := s.parameters[path]
	if !ok {
		return featureflags.Parameter{}, featureflags.ErrParameterNotFound
	}
	return parameter, nil
}
func (s *featureFlagStore) Create(_ context.Context, parameter featureflags.Parameter) (featureflags.Parameter, error) {
	if _, ok := s.parameters[parameter.Name]; ok {
		return featureflags.Parameter{}, featureflags.ErrParameterExists
	}
	parameter.Version = 1
	parameter.LastModified = time.Now().UTC()
	s.parameters[parameter.Name] = parameter
	return parameter, nil
}
func (s *featureFlagStore) Update(_ context.Context, path, value string) (featureflags.Parameter, error) {
	updateErr := s.updateErr
	if updateErr != nil {
		return featureflags.Parameter{}, errors.Join(updateErr)
	}
	parameter, ok := s.parameters[path]
	if !ok {
		return featureflags.Parameter{}, featureflags.ErrParameterNotFound
	}
	s.updatePath = path
	parameter.Value = value
	parameter.Version++
	s.parameters[path] = parameter
	return parameter, nil
}
func (s *featureFlagStore) Delete(_ context.Context, path string) error {
	deleteErr := s.deleteErr
	if deleteErr != nil {
		return errors.Join(deleteErr)
	}
	if _, ok := s.parameters[path]; !ok {
		return featureflags.ErrParameterNotFound
	}
	s.deletePath = path
	delete(s.parameters, path)
	return nil
}

func TestFeatureFlagHandlersMapSecureStringMutationErrors(t *testing.T) {
	parameter := featureflags.Parameter{Name: featureflagadmin.Prefix + "enabled", Value: "true", Type: "String", Version: 1, Tags: map[string]string{"ManagedBy": "cyclops-cs-admin"}}
	for _, testCase := range []struct {
		name   string
		method string
		body   string
		invoke func(Handlers, http.ResponseWriter, *http.Request)
		store  *featureFlagStore
	}{
		{name: "update", method: http.MethodPut, body: `{"value_type":"string","value":"replacement","expected_version":1}`, store: &featureFlagStore{parameters: map[string]featureflags.Parameter{parameter.Name: parameter}, updateErr: featureflags.ErrSecureString}, invoke: func(h Handlers, w http.ResponseWriter, r *http.Request) { h.UpdateFeatureFlag(w, r) }},
		{name: "delete", method: http.MethodDelete, body: `{"expected_version":1}`, store: &featureFlagStore{parameters: map[string]featureflags.Parameter{parameter.Name: parameter}, deleteErr: featureflags.ErrSecureString}, invoke: func(h Handlers, w http.ResponseWriter, r *http.Request) { h.DeleteFeatureFlag(w, r) }},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			h := featureFlagHandlers(t, testCase.store)
			request := featureFlagRequest(testCase.method, "/api/admin/feature-flags/enabled", testCase.body, adminUser())
			request.SetPathValue("key", "enabled")
			response := httptest.NewRecorder()
			testCase.invoke(h, response, request)
			if response.Code != http.StatusUnprocessableEntity {
				t.Fatalf("status = %d, want 422; body = %s", response.Code, response.Body.String())
			}
			var apiError AdminAPIError
			if err := json.NewDecoder(response.Body).Decode(&apiError); err != nil {
				t.Fatal(err)
			}
			if apiError.Code != "unsupported_parameter" {
				t.Fatalf("code = %q, want unsupported_parameter", apiError.Code)
			}
		})
	}
}

type featureFlagLock struct{}

func (featureFlagLock) WithLock(ctx context.Context, callback func(context.Context) error) error {
	return callback(ctx)
}

func featureFlagHandlers(t *testing.T, store *featureFlagStore) Handlers {
	t.Helper()
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-1"]`)
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup provider: %v", err)
	}
	auth.LoadOpa()
	return Handlers{FeatureFlags: featureflagadmin.NewService(store, featureFlagLock{}, nil, nil)}
}
func featureFlagRequest(method, path, body string, user *auth.User) *http.Request {
	request := httptest.NewRequest(method, path, bytes.NewBufferString(body))
	return withUser(request, user)
}
func adminUser() *auth.User {
	return &auth.User{ID: "admin-1", Email: "admin@example.com", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}
}

func TestListFeatureFlagsRejectsNonAdmin(t *testing.T) {
	h := featureFlagHandlers(t, newFeatureFlagStore())
	response := httptest.NewRecorder()
	h.ListFeatureFlags(response, featureFlagRequest(http.MethodGet, "/api/admin/feature-flags", "", &auth.User{ID: "user-1", AZP: "cyclops-cs-spa"}))
	if response.Code != http.StatusForbidden {
		t.Fatalf("status = %d, want 403; body = %s", response.Code, response.Body.String())
	}
	var apiError AdminAPIError
	if err := json.Unmarshal(response.Body.Bytes(), &apiError); err != nil {
		t.Fatal(err)
	}
	if apiError.Code != "not_admin" || apiError.Message != "feature flag administration requires an admin user" {
		t.Fatalf("error = %#v", apiError)
	}
}

func TestFeatureFlagHandlerAdminEvaluationFailureIsStableAndAudited(t *testing.T) {
	var logs bytes.Buffer
	previousLogger := slog.Default()
	slog.SetDefault(slog.New(slog.NewJSONHandler(&logs, nil)))
	t.Cleanup(func() { slog.SetDefault(previousLogger) })
	previousEval := evalFeatureFlagAdmin
	evalFeatureFlagAdmin = func(context.Context, *auth.User) (bool, error) { return false, errors.New("opa unavailable") }
	t.Cleanup(func() { evalFeatureFlagAdmin = previousEval })

	h := featureFlagHandlers(t, newFeatureFlagStore())
	response := httptest.NewRecorder()
	request := featureFlagRequest(http.MethodPost, "/api/admin/feature-flags", `{"key":"enabled","value_type":"boolean","value":true}`, adminUser())
	h.CreateFeatureFlag(response, request)
	if response.Code != http.StatusInternalServerError {
		t.Fatalf("status = %d, want 500; body = %s", response.Code, response.Body.String())
	}
	var apiError AdminAPIError
	if err := json.Unmarshal(response.Body.Bytes(), &apiError); err != nil {
		t.Fatal(err)
	}
	if apiError.Code != "policy_error" || apiError.Message == "" {
		t.Fatalf("error = %#v", apiError)
	}
	if count := strings.Count(logs.String(), `"event":"feature_flag_admin"`); count != 1 || !strings.Contains(logs.String(), `"reason":"policy_error"`) {
		t.Fatalf("audit logs = %s", logs.String())
	}
}

func TestUpdateFeatureFlagDefenseInDepthDenialAuditsExactlyOnce(t *testing.T) {
	var logs bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewJSONHandler(&logs, nil)))
	t.Cleanup(func() { slog.SetDefault(previous) })

	h := featureFlagHandlers(t, newFeatureFlagStore())
	response := httptest.NewRecorder()
	invalidKey := strings.Repeat("Sensitive/", 40)
	requestPath := "/api/admin/feature-flags/" + invalidKey
	request := featureFlagRequest(http.MethodPut, requestPath, `{"value_type":"boolean","value":true,"expected_version":1}`, &auth.User{
		ID: "user-1", Email: "user@example.com", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser,
	})
	request.SetPathValue("key", invalidKey)
	request = request.WithContext(context.WithValue(request.Context(), middlewares.ContextKey("traceId"), "trace-123"))
	h.UpdateFeatureFlag(response, request)

	if response.Code != http.StatusForbidden {
		t.Fatalf("status = %d, want 403; body = %s", response.Code, response.Body.String())
	}
	var apiError AdminAPIError
	if err := json.Unmarshal(response.Body.Bytes(), &apiError); err != nil {
		t.Fatal(err)
	}
	if apiError.Code != "not_admin" {
		t.Fatalf("error = %#v", apiError)
	}

	events := make([]map[string]any, 0, 1)
	decoder := json.NewDecoder(bytes.NewReader(logs.Bytes()))
	for {
		var record map[string]any
		if err := decoder.Decode(&record); err == io.EOF {
			break
		} else if err != nil {
			t.Fatalf("decode logs: %v; logs=%s", err, logs.String())
		}
		if record["event"] == "feature_flag_admin" {
			events = append(events, record)
		}
	}
	if len(events) != 1 {
		t.Fatalf("feature flag audit events = %d, want 1; logs=%s", len(events), logs.String())
	}
	for key, want := range map[string]any{
		"actor": "user-1", "actor_email": "user@example.com", "principal_type": "user",
		"operation": "put", "traceId": "trace-123", "result": "rejected", "reason": "not_admin",
	} {
		if got := events[0][key]; got != want {
			t.Errorf("%s = %#v, want %#v", key, got, want)
		}
	}
	for _, field := range []string{"key", "path"} {
		value, _ := events[0][field].(string)
		if !strings.HasPrefix(value, "sha256:") || len(value) > 80 {
			t.Errorf("%s = %q, want bounded SHA-256 identifier", field, value)
		}
	}
	if strings.Contains(logs.String(), invalidKey) {
		t.Fatalf("audit leaked unvalidated key/path: %s", logs.String())
	}
}

func TestCreateFeatureFlagReturns201(t *testing.T) {
	store := newFeatureFlagStore()
	h := featureFlagHandlers(t, store)
	response := httptest.NewRecorder()
	h.CreateFeatureFlag(response, featureFlagRequest(http.MethodPost, "/api/admin/feature-flags", `{"key":"enabled","value_type":"boolean","value":true}`, adminUser()))
	if response.Code != http.StatusCreated {
		t.Fatalf("status = %d, want 201; body = %s", response.Code, response.Body.String())
	}
	if _, ok := store.parameters[featureflagadmin.Prefix+"enabled"]; !ok {
		t.Fatal("flag was not created")
	}
}

func TestUpdateFeatureFlagReturnsCurrentOnVersionConflict(t *testing.T) {
	current := featureflags.Parameter{Name: featureflagadmin.Prefix + "enabled", Value: "true", Type: "String", Version: 3, LastModified: time.Now().UTC(), Tags: map[string]string{"ManagedBy": "cyclops-cs-admin"}}
	h := featureFlagHandlers(t, newFeatureFlagStore(current))
	response := httptest.NewRecorder()
	request := featureFlagRequest(http.MethodPut, "/api/admin/feature-flags/enabled", `{"value_type":"boolean","value":false,"expected_version":2}`, adminUser())
	request.SetPathValue("key", "enabled")
	h.UpdateFeatureFlag(response, request)
	if response.Code != http.StatusConflict {
		t.Fatalf("status = %d, want 409; body = %s", response.Code, response.Body.String())
	}
	var apiError AdminAPIError
	if err := json.NewDecoder(response.Body).Decode(&apiError); err != nil {
		t.Fatal(err)
	}
	if apiError.Code != "version_conflict" || apiError.Current == nil || apiError.Current.Version != 3 {
		t.Fatalf("error = %#v", apiError)
	}
}

func TestDeleteFeatureFlagAcceptsExpectedVersionBody(t *testing.T) {
	parameter := featureflags.Parameter{Name: featureflagadmin.Prefix + "enabled", Value: "true", Type: "String", Version: 3, LastModified: time.Now().UTC(), Tags: map[string]string{"ManagedBy": "cyclops-cs-admin"}}
	store := newFeatureFlagStore(parameter)
	h := featureFlagHandlers(t, store)
	response := httptest.NewRecorder()
	request := featureFlagRequest(http.MethodDelete, "/api/admin/feature-flags/enabled", `{"expected_version":3}`, adminUser())
	request.SetPathValue("key", "enabled")
	h.DeleteFeatureFlag(response, request)
	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want 204; body = %s", response.Code, response.Body.String())
	}
	if store.deletePath != featureflagadmin.Prefix+"enabled" {
		t.Fatalf("delete path = %q", store.deletePath)
	}
}

func TestDeleteFeatureFlagRejectsAdminSubsWithLastAdmin(t *testing.T) {
	parameter := featureflags.Parameter{Name: featureflagadmin.Prefix + "admin-subs", Value: `["admin-1"]`, Type: "String", Version: 3, LastModified: time.Now().UTC(), Tags: map[string]string{"ManagedBy": "cyclops-cs-admin"}}
	store := newFeatureFlagStore(parameter)
	h := featureFlagHandlers(t, store)
	response := httptest.NewRecorder()
	request := featureFlagRequest(http.MethodDelete, "/api/admin/feature-flags/admin-subs", `{"expected_version":3}`, adminUser())
	request.SetPathValue("key", "admin-subs")
	h.DeleteFeatureFlag(response, request)
	if response.Code != http.StatusUnprocessableEntity {
		t.Fatalf("status = %d, want 422; body = %s", response.Code, response.Body.String())
	}
	var apiError AdminAPIError
	if err := json.NewDecoder(response.Body).Decode(&apiError); err != nil {
		t.Fatal(err)
	}
	if apiError.Code != "last_admin" {
		t.Fatalf("code = %q, want last_admin", apiError.Code)
	}
	if store.deletePath != "" {
		t.Fatalf("delete path = %q, want no deletion", store.deletePath)
	}
}

func TestFeatureFlagHandlersReturnStableErrors(t *testing.T) {
	h := featureFlagHandlers(t, newFeatureFlagStore())
	response := httptest.NewRecorder()
	h.CreateFeatureFlag(response, featureFlagRequest(http.MethodPost, "/api/admin/feature-flags", `{"key":"Bad_Key","value_type":"boolean","value":true}`, adminUser()))
	if response.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400", response.Code)
	}
	var apiError AdminAPIError
	if err := json.NewDecoder(response.Body).Decode(&apiError); err != nil {
		t.Fatal(err)
	}
	if apiError.Code != "invalid_key" || apiError.Message == "" {
		t.Fatalf("error = %#v", apiError)
	}
}

func TestFeatureFlagHandlersReturnUnsupportedProvider(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-1"]`)
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup provider: %v", err)
	}
	auth.LoadOpa()
	response := httptest.NewRecorder()
	Handlers{FeatureFlags: featureflagadmin.NewUnsupportedService(nil)}.ListFeatureFlags(response, featureFlagRequest(http.MethodGet, "/api/admin/feature-flags", "", adminUser()))
	if response.Code != http.StatusNotImplemented {
		t.Fatalf("status = %d, want 501; body = %s", response.Code, response.Body.String())
	}
	var apiError AdminAPIError
	if err := json.NewDecoder(response.Body).Decode(&apiError); err != nil {
		t.Fatal(err)
	}
	if apiError.Code != "unsupported_provider" {
		t.Fatalf("code = %q", apiError.Code)
	}
}

func TestFeatureFlagHandlersAuditStrictJSONRejectionsWithoutRawBodies(t *testing.T) {
	validCreate := `{"key":"enabled","value_type":"boolean","value":true}`
	cases := []struct {
		name         string
		method       string
		path         string
		key          string
		body         string
		parseFailure string
		invoke       func(Handlers, http.ResponseWriter, *http.Request)
	}{
		{name: "malformed create", method: http.MethodPost, path: "/api/admin/feature-flags", body: `{"key":"secret-body-value"`, parseFailure: "malformed_json", invoke: func(h Handlers, w http.ResponseWriter, r *http.Request) { h.CreateFeatureFlag(w, r) }},
		{name: "oversized create", method: http.MethodPost, path: "/api/admin/feature-flags", body: validCreate + strings.Repeat("X", maxFeatureFlagRequestBytes-len(validCreate)+1), parseFailure: "body_too_large", invoke: func(h Handlers, w http.ResponseWriter, r *http.Request) { h.CreateFeatureFlag(w, r) }},
		{name: "unknown update field", method: http.MethodPut, path: "/api/admin/feature-flags/enabled", key: "enabled", body: `{"value_type":"boolean","value":true,"expected_version":1,"unexpected":"secret-body-value"}`, parseFailure: "unknown_field", invoke: func(h Handlers, w http.ResponseWriter, r *http.Request) { h.UpdateFeatureFlag(w, r) }},
		{name: "trailing delete data", method: http.MethodDelete, path: "/api/admin/feature-flags/enabled", key: "enabled", body: `{"expected_version":1} {"secret":"secret-body-value"}`, parseFailure: "trailing_data", invoke: func(h Handlers, w http.ResponseWriter, r *http.Request) { h.DeleteFeatureFlag(w, r) }},
	}
	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			var logs bytes.Buffer
			previous := slog.Default()
			slog.SetDefault(slog.New(slog.NewJSONHandler(&logs, nil)))
			t.Cleanup(func() { slog.SetDefault(previous) })

			h := featureFlagHandlers(t, newFeatureFlagStore())
			response := httptest.NewRecorder()
			request := featureFlagRequest(testCase.method, testCase.path, testCase.body, adminUser())
			request.SetPathValue("key", testCase.key)
			request = request.WithContext(context.WithValue(request.Context(), middlewares.ContextKey("traceId"), "trace-123"))
			testCase.invoke(h, response, request)
			if response.Code != http.StatusBadRequest {
				t.Fatalf("status = %d, want 400; body = %s", response.Code, response.Body.String())
			}
			var record map[string]any
			decoder := json.NewDecoder(bytes.NewReader(logs.Bytes()))
			for {
				var candidate map[string]any
				if err := decoder.Decode(&candidate); err == io.EOF {
					break
				} else if err != nil {
					t.Fatalf("unmarshal audit log: %v; logs=%s", err, logs.String())
				}
				if candidate["event"] == "feature_flag_admin" {
					record = candidate
				}
			}
			if record == nil {
				t.Fatalf("feature flag audit missing: %s", logs.String())
			}
			for key, want := range map[string]any{
				"event": "feature_flag_admin", "actor": "admin-1", "actor_email": "admin@example.com",
				"principal_type": "user", "operation": strings.ToLower(testCase.method), "key": testCase.key,
				"path": testCase.path, "traceId": "trace-123", "result": "rejected", "reason": "invalid_request",
				"parse_failure": testCase.parseFailure,
			} {
				if got := record[key]; got != want {
					t.Errorf("%s = %#v, want %#v; logs=%s", key, got, want, logs.String())
				}
			}
			if strings.Contains(logs.String(), "secret-body-value") || strings.Contains(logs.String(), strings.Repeat("X", 128)) {
				t.Fatalf("audit leaked raw request body: %s", logs.String())
			}
		})
	}
}

func TestFeatureFlagHandlerReturnsLeaseNotFound(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-1"]`)
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup provider: %v", err)
	}
	auth.LoadOpa()
	service := featureflagadmin.NewService(newFeatureFlagStore(), errorLock{err: featureflagadmin.ErrLeaseNotFound}, nil, nil)
	response := httptest.NewRecorder()
	Handlers{FeatureFlags: service}.CreateFeatureFlag(response, featureFlagRequest(http.MethodPost, "/api/admin/feature-flags", `{"key":"enabled","value_type":"boolean","value":true}`, adminUser()))
	if response.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, want 503; body = %s", response.Code, response.Body.String())
	}
	var apiError AdminAPIError
	if err := json.NewDecoder(response.Body).Decode(&apiError); err != nil {
		t.Fatal(err)
	}
	if apiError.Code != "lease_not_found" {
		t.Fatalf("code = %q", apiError.Code)
	}
}

type errorLock struct{ err error }

func (l errorLock) WithLock(context.Context, func(context.Context) error) error { return l.err }

func TestFeatureFlagHandlerNilStoreReturnsUnsupportedProvider(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["admin-1"]`)
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup provider: %v", err)
	}
	auth.LoadOpa()
	response := httptest.NewRecorder()
	Handlers{FeatureFlags: featureflagadmin.NewService(nil, featureFlagLock{}, nil, nil)}.ListFeatureFlags(response, featureFlagRequest(http.MethodGet, "/api/admin/feature-flags", "", adminUser()))
	if response.Code != http.StatusNotImplemented {
		t.Fatalf("status = %d, want 501; body = %s", response.Code, response.Body.String())
	}
	var apiError AdminAPIError
	if err := json.NewDecoder(response.Body).Decode(&apiError); err != nil {
		t.Fatal(err)
	}
	if apiError.Code != "unsupported_provider" {
		t.Fatalf("code = %q", apiError.Code)
	}
}
