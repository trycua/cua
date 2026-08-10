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
	"github.com/jackc/pgx/v5/pgconn"
)

type fakeStateQueryExecutor struct {
	tenant string
	query  string
	fields []pgconn.FieldDescription
	rows   [][]any
	err    error
}

func (fake *fakeStateQueryExecutor) Execute(_ context.Context, tenant, query string, writer statequery.ResultWriter) error {
	fake.tenant, fake.query = tenant, query
	if err := writer.WriteFieldDescriptions(fake.fields); err != nil {
		return err
	}
	for _, row := range fake.rows {
		if err := writer.WriteRow(row); err != nil {
			return err
		}
	}
	return fake.err
}

func TestQueryStateReturnsUnavailableWithoutExecutor(t *testing.T) {
	h := Handlers{}
	request := withUser(httptest.NewRequest("QUERY", "/api/state/query", strings.NewReader("select 1")), &auth.User{ID: "alice"})
	request.Header.Set("Content-Type", "application/sql")
	response := httptest.NewRecorder()
	h.QueryState(response, request)
	if response.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
	}
	if got := response.Header().Get("Accept-Query"); got != "application/sql" {
		t.Fatalf("Accept-Query = %q", got)
	}
}

func TestQueryStateStreamsRawQueryResults(t *testing.T) {
	executor := &fakeStateQueryExecutor{
		fields: []pgconn.FieldDescription{{Name: "name", DataTypeOID: 25}},
		rows:   [][]any{{"alice-ns"}, {"alice-pod"}},
	}
	h := Handlers{StateQueryExecutor: executor}
	query := "select pg_sleep(60)"
	request := withUser(httptest.NewRequest("QUERY", "/api/state/query", strings.NewReader(query)), &auth.User{ID: "alice"})
	request.Header.Set("Content-Type", "application/sql; charset=utf-8")
	response := httptest.NewRecorder()
	h.QueryState(response, request)
	if response.Code != http.StatusOK {
		t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
	}
	if executor.tenant != "user-alice" || executor.query != query {
		t.Fatalf("tenant/query = %q/%q", executor.tenant, executor.query)
	}
	if got := response.Header().Get("Content-Type"); got != "application/x-ndjson" {
		t.Fatalf("Content-Type = %q", got)
	}
	want := "" +
		`{"type":"fields","fields":[{"Name":"name","TableOID":0,"TableAttributeNumber":0,"DataTypeOID":25,"DataTypeSize":0,"TypeModifier":0,"Format":0}]}` + "\n" +
		`{"type":"row","values":["alice-ns"]}` + "\n" +
		`{"type":"row","values":["alice-pod"]}` + "\n" +
		`{"type":"done"}` + "\n"
	if response.Body.String() != want {
		t.Fatalf("body = %q, want %q", response.Body.String(), want)
	}
	if !response.Flushed {
		t.Fatal("stream was not flushed")
	}
}

func TestQueryStateRequiresSQLContentType(t *testing.T) {
	h := Handlers{StateQueryExecutor: &fakeStateQueryExecutor{}}
	request := withUser(httptest.NewRequest("QUERY", "/api/state/query", strings.NewReader("select 1")), &auth.User{ID: "alice"})
	response := httptest.NewRecorder()
	h.QueryState(response, request)
	if response.Code != http.StatusUnsupportedMediaType {
		t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
	}
}

func TestQueryStateWritesExecutorErrorAsTerminalEvent(t *testing.T) {
	executor := &fakeStateQueryExecutor{
		fields: []pgconn.FieldDescription{{Name: "name", DataTypeOID: 25}},
		rows:   [][]any{{"alice-ns"}},
		err:    errors.New("stream failed"),
	}
	h := Handlers{StateQueryExecutor: executor}
	request := withUser(httptest.NewRequest("QUERY", "/api/state/query", strings.NewReader("select 1")), &auth.User{ID: "alice"})
	request.Header.Set("Content-Type", "application/sql")
	response := httptest.NewRecorder()
	h.QueryState(response, request)
	if response.Code != http.StatusOK {
		t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
	}
	wantSuffix := `{"type":"error","error":"stream failed"}` + "\n"
	if !strings.HasSuffix(response.Body.String(), wantSuffix) {
		t.Fatalf("body = %q, want suffix %q", response.Body.String(), wantSuffix)
	}
}
