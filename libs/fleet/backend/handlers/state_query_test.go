package handlers

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

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

type deadlineStateQueryExecutor struct {
	context context.Context
	write   bool
	err     error
}

func (fake *deadlineStateQueryExecutor) Execute(ctx context.Context, _ string, _ string, writer statequery.ResultWriter) error {
	fake.context = ctx
	if fake.write {
		if err := writer.WriteFieldDescriptions([]pgconn.FieldDescription{{Name: "name", DataTypeOID: 25}}); err != nil {
			return err
		}
	}
	return fake.err
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

func TestQueryStateUsesBoundedDatabaseContext(t *testing.T) {
	tests := []struct {
		name            string
		write           bool
		err             error
		wantStatus      int
		wantBody        string
		wantSuffix      string
		wantUnavailable bool
	}{
		{"deadline before stream start", false, context.DeadlineExceeded, http.StatusServiceUnavailable, "", "", true},
		{"executor error before stream start", false, errors.New("stream failed"), http.StatusServiceUnavailable, "", "", true},
		{"deadline after stream start", true, context.DeadlineExceeded, http.StatusOK, "", `{"type":"error","error":"state query unavailable"}` + "\n", true},
		{"executor error after stream start", true, errors.New("stream failed"), http.StatusOK, "", `{"type":"error","error":"stream failed"}` + "\n", false},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			executor := &deadlineStateQueryExecutor{write: test.write, err: test.err}
			h := Handlers{StateQueryExecutor: executor}
			request := withUser(httptest.NewRequest("QUERY", "/api/state/query", strings.NewReader("select 1")), &auth.User{ID: "alice"})
			request.Header.Set("Content-Type", "application/sql")
			response := httptest.NewRecorder()
			start := time.Now()

			h.QueryState(response, request)

			assertBoundedDatabaseContext(t, executor.context, start)
			if response.Code != test.wantStatus {
				t.Fatalf("status = %d, want %d; body = %s", response.Code, test.wantStatus, response.Body.String())
			}
			if test.wantBody != "" && response.Body.String() != test.wantBody {
				t.Fatalf("body = %q, want %q", response.Body.String(), test.wantBody)
			}
			if test.wantSuffix != "" && !strings.HasSuffix(response.Body.String(), test.wantSuffix) {
				t.Fatalf("body = %q, want suffix %q", response.Body.String(), test.wantSuffix)
			}
			if test.wantUnavailable && strings.Contains(response.Body.String(), test.err.Error()) {
				t.Fatalf("body exposes internal error: %q", response.Body.String())
			}
		})
	}
}

func TestQueryStateDatabaseFailureBeforeStreamReturnsServiceUnavailable(t *testing.T) {
	executor := &deadlineStateQueryExecutor{err: auth.ErrDatabaseUnavailable}
	h := Handlers{StateQueryExecutor: executor}
	request := withUser(httptest.NewRequest("QUERY", "/api/state/query", strings.NewReader("select 1")), &auth.User{ID: "alice"})
	request.Header.Set("Content-Type", "application/sql")
	response := httptest.NewRecorder()

	h.QueryState(response, request)

	if response.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
	}
	if strings.Contains(response.Body.String(), auth.ErrDatabaseUnavailable.Error()) {
		t.Fatalf("body exposes internal error: %s", response.Body.String())
	}
}

func TestQueryStateDatabaseFailureAfterStreamWritesSanitizedTerminalEvent(t *testing.T) {
	executor := &deadlineStateQueryExecutor{write: true, err: auth.ErrDatabaseUnavailable}
	h := Handlers{StateQueryExecutor: executor}
	request := withUser(httptest.NewRequest("QUERY", "/api/state/query", strings.NewReader("select 1")), &auth.User{ID: "alice"})
	request.Header.Set("Content-Type", "application/sql")
	response := httptest.NewRecorder()

	h.QueryState(response, request)

	if response.Code != http.StatusOK {
		t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
	}
	if !strings.HasSuffix(response.Body.String(), `{"type":"error","error":"state query unavailable"}`+"\n") {
		t.Fatalf("body = %q", response.Body.String())
	}
}

type cancellationStateQueryExecutor struct {
	started chan<- struct{}
	done    chan<- struct{}
}

func (executor cancellationStateQueryExecutor) Execute(ctx context.Context, _, _ string, _ statequery.ResultWriter) error {
	close(executor.started)
	<-ctx.Done()
	close(executor.done)
	return ctx.Err()
}

func TestQueryStatePropagatesRequestCancellation(t *testing.T) {
	started := make(chan struct{})
	done := make(chan struct{})
	handlerDone := make(chan struct{})
	h := Handlers{StateQueryExecutor: cancellationStateQueryExecutor{started: started, done: done}}
	parent, cancel := context.WithCancel(context.Background())
	defer cancel()
	request := withUser(httptest.NewRequest("QUERY", "/api/state/query", strings.NewReader("select 1")).WithContext(parent), &auth.User{ID: "alice"})
	request.Header.Set("Content-Type", "application/sql")
	response := httptest.NewRecorder()

	go func() {
		h.QueryState(response, request)
		close(handlerDone)
	}()
	<-started
	cancel()
	<-done
	<-handlerDone
	if response.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
	}
}
