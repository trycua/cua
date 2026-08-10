package auth

import (
	"bytes"
	"context"
	"encoding/json"
	"log/slog"
	"net/http"
	"strings"
)

// k8sEventListPath returns true when the upstream K8s API path targets the
// core/v1 Event resource — either cluster-scoped (/api/v1/events[/name]) or
// namespaced (/api/v1/namespaces/{ns}/events[/name]). Other K8s API verbs
// and resources pass through the K8s reverse proxy unfiltered.
//
// `path` has a leading slash and no query string (the caller derives it
// from r.PathValue("path")). A substring check is deliberately avoided: it
// would miss the namespaced form (which does not contain "/api/v1/events")
// and so would let namespaced event lists bypass the visibility filter.
func k8sEventListPath(path string) bool {
	if path == "/api/v1/events" || strings.HasPrefix(path, "/api/v1/events/") {
		return true
	}
	if rest, ok := strings.CutPrefix(path, "/api/v1/namespaces/"); ok {
		if _, after, ok := strings.Cut(rest, "/"); ok {
			return after == "events" || strings.HasPrefix(after, "events/")
		}
	}
	return false
}

// isWatch reports whether the request is a K8s watch (streaming) request,
// which must not be buffered by the event filter.
func isWatch(r *http.Request) bool {
	v := r.URL.Query().Get("watch")
	return v == "true" || v == "1"
}

// K8sEventFilterMiddleware applies filters.rego's visible_events rule
// to K8s EventList responses returned by the /api/k8s reverse proxy.
// Requests whose upstream path is not an EventList pass through with
// zero overhead; matching responses get buffered, parsed as JSON,
// filtered through OPA, and rewritten before reaching the client.
//
// Mount this after RouteContext (so r.PathValue("path") is populated)
// and after TokenAuthMiddleware (so GetUser returns the caller). The
// filter is opt-in per-route: the K8s handler is the only consumer
// today.
//
// Non-200, non-JSON, OPA-error, and gzip-encoded responses pass through
// unchanged — the original behaviour of filterEventListResponse before
// this was extracted from handlers/k8s.go.
func K8sEventFilterMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upstream := "/" + strings.TrimPrefix(r.PathValue("path"), "/")
		if !k8sEventListPath(upstream) {
			next.ServeHTTP(w, r)
			return
		}
		// A watch is a long-lived chunked stream, not a single EventList.
		// Buffering it would hang the request and grow memory unboundedly,
		// so stream it straight through (the SPA only polls list GETs).
		if isWatch(r) {
			next.ServeHTTP(w, r)
			return
		}
		user := GetUser(r.Context())
		buf := &eventFilterBuffer{ResponseWriter: w, filter: EvalVisibleEvents}
		next.ServeHTTP(buf, r)
		buf.flush(r.Context(), user)
	})
}

// eventFilterBuffer captures status + body so the middleware can decide
// whether to forward the original bytes or replace them with a filtered
// EventList. Header() returns the underlying writer's map (via the
// embedded ResponseWriter), so the inner handler keeps setting headers
// as usual; only the WriteHeader commit is delayed until flush().
//
// filter is the OPA-backed visibility function (EvalVisibleEvents in
// production); it is a field so tests can inject a stub — in particular to
// exercise the fail-closed path when it returns an error.
type eventFilterBuffer struct {
	http.ResponseWriter
	filter    func(context.Context, *User, []interface{}) ([]interface{}, error)
	status    int
	wroteOnce bool
	body      bytes.Buffer
}

func (b *eventFilterBuffer) WriteHeader(status int) {
	if b.wroteOnce {
		return
	}
	b.status = status
	b.wroteOnce = true
}

func (b *eventFilterBuffer) Write(p []byte) (int, error) {
	if !b.wroteOnce {
		b.WriteHeader(http.StatusOK)
	}
	return b.body.Write(p)
}

func (b *eventFilterBuffer) flush(ctx context.Context, user *User) {
	h := b.ResponseWriter.Header()
	status := b.status
	if status == 0 {
		status = http.StatusOK
	}
	passthrough := func() {
		b.ResponseWriter.WriteHeader(status)
		if _, err := b.ResponseWriter.Write(b.body.Bytes()); err != nil {
			slog.Debug("k8s event filter: passthrough write error", "err", err)
		}
	}
	if status != http.StatusOK {
		passthrough()
		return
	}
	var eventList map[string]interface{}
	if err := json.Unmarshal(b.body.Bytes(), &eventList); err != nil {
		// Not JSON (e.g. gzip-encoded upstream) — forward as-is so the
		// client decodes it normally.
		passthrough()
		return
	}
	items, _ := eventList["items"].([]interface{})
	if items == nil {
		passthrough()
		return
	}
	filtered, err := b.filter(ctx, user, items)
	if err != nil {
		// Fail closed: on policy-eval error we cannot tell whether the
		// caller is an admin, so redact ALL events rather than risk
		// leaking internal K8s event data to a non-admin. The SPA renders
		// an empty (rather than unfiltered) list.
		slog.Warn("opa: visible_events eval failed; redacting all events (fail-closed)", "err", err)
		filtered = []interface{}{}
	}
	eventList["items"] = filtered
	out, err := json.Marshal(eventList)
	if err != nil {
		passthrough()
		return
	}
	h.Del("Content-Length")
	h.Del("Content-Encoding") // we replaced the body with plain JSON
	h.Set("Content-Type", "application/json")
	b.ResponseWriter.WriteHeader(status)
	if _, err := b.ResponseWriter.Write(out); err != nil {
		slog.Debug("k8s event filter: filtered write error", "err", err)
	}
}
