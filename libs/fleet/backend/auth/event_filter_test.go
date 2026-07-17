package auth

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
)

func eventListJSON(t *testing.T, reasons ...string) []byte {
	t.Helper()
	items := make([]any, 0, len(reasons))
	for _, r := range reasons {
		items = append(items, map[string]any{"reason": r})
	}
	b, err := json.Marshal(map[string]any{"kind": "EventList", "items": items})
	if err != nil {
		t.Fatalf("marshal event list: %v", err)
	}
	return b
}

// runFlush drives an eventFilterBuffer the way the middleware does: write the
// upstream status+body, then flush through the injected filter. It returns the
// committed status and body seen by the client.
func runFlush(status int, body []byte, filter func(context.Context, *User, []any) ([]any, error)) (int, []byte, http.Header) {
	rec := httptest.NewRecorder()
	buf := &eventFilterBuffer{ResponseWriter: rec, filter: filter}
	if status != 0 {
		buf.WriteHeader(status)
	}
	_, _ = buf.Write(body)
	buf.flush(context.Background(), &User{ID: "u-1"})
	res := rec.Result()
	defer res.Body.Close()
	out, _ := io.ReadAll(res.Body)
	return res.StatusCode, out, res.Header
}

func itemsLen(t *testing.T, body []byte) int {
	t.Helper()
	var m map[string]any
	if err := json.Unmarshal(body, &m); err != nil {
		t.Fatalf("output not JSON: %v (%s)", err, body)
	}
	items, ok := m["items"].([]any)
	if !ok {
		t.Fatalf("output items missing/not array: %s", body)
	}
	return len(items)
}

func TestEventFilter_FailClosedOnError(t *testing.T) {
	// The crux: if the policy eval errors, the client must NOT receive the
	// unfiltered events. We expect an empty items array, not a passthrough.
	called := false
	stub := func(context.Context, *User, []any) ([]any, error) {
		called = true
		return nil, errors.New("opa boom")
	}
	status, body, _ := runFlush(http.StatusOK, eventListJSON(t, "PoolCreated", "InternalScaleDecision"), stub)
	if !called {
		t.Fatal("filter was not invoked")
	}
	if status != http.StatusOK {
		t.Fatalf("status = %d, want 200", status)
	}
	if n := itemsLen(t, body); n != 0 {
		t.Fatalf("fail-closed should redact all events, got %d items", n)
	}
}

func TestEventFilter_AdminPassthroughAndSubset(t *testing.T) {
	all := func(_ context.Context, _ *User, items []any) ([]any, error) {
		return items, nil
	}
	_, body, _ := runFlush(http.StatusOK, eventListJSON(t, "A", "B", "C"), all)
	if n := itemsLen(t, body); n != 3 {
		t.Fatalf("admin passthrough = %d items, want 3", n)
	}

	firstOnly := func(_ context.Context, _ *User, items []any) ([]any, error) {
		if len(items) == 0 {
			return items, nil
		}
		return items[:1], nil
	}
	_, body, _ = runFlush(http.StatusOK, eventListJSON(t, "A", "B", "C"), firstOnly)
	if n := itemsLen(t, body); n != 1 {
		t.Fatalf("subset filter = %d items, want 1", n)
	}
}

func TestEventFilter_NonOKPassesThroughUnfiltered(t *testing.T) {
	called := false
	stub := func(context.Context, *User, []any) ([]any, error) {
		called = true
		return nil, nil
	}
	original := []byte(`{"kind":"Status","status":"Failure","code":500}`)
	status, body, _ := runFlush(http.StatusInternalServerError, original, stub)
	if called {
		t.Fatal("filter must not run on non-200 responses")
	}
	if status != http.StatusInternalServerError {
		t.Fatalf("status = %d, want 500", status)
	}
	if string(body) != string(original) {
		t.Fatalf("body altered on passthrough: %s", body)
	}
}

func TestEventFilter_NonJSONPassesThrough(t *testing.T) {
	called := false
	stub := func(context.Context, *User, []any) ([]any, error) {
		called = true
		return nil, nil
	}
	original := []byte("\x1f\x8b not json (e.g. gzip)")
	_, body, _ := runFlush(http.StatusOK, original, stub)
	if called {
		t.Fatal("filter must not run on non-JSON bodies")
	}
	if string(body) != string(original) {
		t.Fatalf("body altered: %s", body)
	}
}

func TestEventFilter_NoItemsFieldPassesThrough(t *testing.T) {
	called := false
	stub := func(context.Context, *User, []any) ([]any, error) {
		called = true
		return nil, nil
	}
	original := []byte(`{"kind":"Pod","metadata":{"name":"p"}}`)
	_, body, _ := runFlush(http.StatusOK, original, stub)
	if called {
		t.Fatal("filter must not run when there is no items array")
	}
	if string(body) != string(original) {
		t.Fatalf("body altered: %s", body)
	}
}

func TestK8sEventListPath(t *testing.T) {
	cases := map[string]bool{
		"/api/v1/events":                      true,
		"/api/v1/namespaces/pool-foo/events":  true,
		"/api/v1/nodes":                       false,
		"/api/v1/namespaces/pool-foo/pods":    false,
		"/apis/cua.ai/v1/osgymworkspacepools": false,
	}
	for path, want := range cases {
		if got := k8sEventListPath(path); got != want {
			t.Fatalf("k8sEventListPath(%q) = %v, want %v", path, got, want)
		}
	}
}
