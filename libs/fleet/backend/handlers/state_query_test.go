package handlers

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/statequery"
)

type fakeStateQueryExecutor struct {
	tenant string
	admin  bool
	query  statequery.ValidatedQuery
	result statequery.ResultSet
	err    error
}

func (fake *fakeStateQueryExecutor) Execute(_ context.Context, tenant string, admin bool, query statequery.ValidatedQuery) (statequery.ResultSet, error) {
	fake.tenant, fake.admin, fake.query = tenant, admin, query
	return fake.result, fake.err
}

func TestQueryStateReturnsUnavailableWithoutExecutor(t *testing.T) {
	h := Handlers{}
	request := withUser(httptest.NewRequest(http.MethodPost, "/api/state/query", strings.NewReader(`{"sql":"select 1"}`)), &auth.User{ID: "alice"})
	response := httptest.NewRecorder()
	h.QueryState(response, request)
	if response.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
	}
}

func TestQueryStateRejectsUnsafeSQL(t *testing.T) {
	executor := &fakeStateQueryExecutor{}
	h := Handlers{StateQueryExecutor: executor}
	request := withUser(httptest.NewRequest(http.MethodPost, "/api/state/query", strings.NewReader(`{"sql":"select pg_sleep(60)"}`)), &auth.User{ID: "alice"})
	response := httptest.NewRecorder()
	h.QueryState(response, request)
	if response.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
	}
}

func TestQueryStateUsesAuthenticatedCapsuleTenant(t *testing.T) {
	executor := &fakeStateQueryExecutor{result: statequery.ResultSet{Rows: [][]any{{"alice-ns"}}}}
	h := Handlers{
		StateQueryExecutor:  executor,
		StateQueryAuthorize: func(context.Context, *auth.User) (bool, error) { return false, nil },
	}
	request := withUser(httptest.NewRequest(http.MethodPost, "/api/state/query", strings.NewReader(`{"sql":"select name from k8s_api.current_resources"}`)), &auth.User{ID: "alice"})
	response := httptest.NewRecorder()
	h.QueryState(response, request)
	if response.Code != http.StatusOK {
		t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
	}
	if executor.tenant != "user-alice" || executor.admin {
		t.Fatalf("tenant/admin = %q/%t", executor.tenant, executor.admin)
	}
}

func TestQueryStateFailsClosedOnAuthorizationError(t *testing.T) {
	executor := &fakeStateQueryExecutor{}
	h := Handlers{
		StateQueryExecutor:  executor,
		StateQueryAuthorize: func(context.Context, *auth.User) (bool, error) { return false, errors.New("opa unavailable") },
	}
	request := withUser(httptest.NewRequest(http.MethodPost, "/api/state/query", strings.NewReader(`{"sql":"select name from k8s_api.current_resources"}`)), &auth.User{ID: "alice"})
	response := httptest.NewRecorder()
	h.QueryState(response, request)
	if response.Code != http.StatusForbidden {
		t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
	}
}
