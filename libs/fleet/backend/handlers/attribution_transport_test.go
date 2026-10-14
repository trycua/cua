package handlers

import (
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"testing/iotest"
	"time"
)

func TestQualificationReusesConnectionAcrossBoundedFactReads(t *testing.T) {
	_, connections := attributionConnectionFixture(t)
	reader := fleetAttributionFactsReader{handlers: Handlers{}}
	for range 2 {
		r := attributionDiagnosticRequest()
		result := QualifySvcRequestResult(r.Context(), r, http.StatusOK, reader)
		if !result.Qualifies {
			t.Fatalf("valid facts must qualify: %#v", result)
		}
	}
	if got := connections.Load(); got != 1 {
		t.Fatalf("six bounded sequential fact reads opened %d connections; want one reused connection", got)
	}
}

func TestQualificationAvoidsRedialThatWouldExhaustSharedDeadline(t *testing.T) {
	transport, _ := attributionConnectionFixture(t)
	r := attributionDiagnosticRequest()
	ctx, cancel := context.WithTimeout(r.Context(), 250*time.Millisecond)
	defer cancel()
	var dials atomic.Int32
	dial := transport.DialContext
	transport.DialContext = func(dialCtx context.Context, network, address string) (net.Conn, error) {
		if dials.Add(1) > 1 {
			// Model a fresh connection that cannot complete within the shared
			// budget. An existing connection remains usable without this delay.
			<-ctx.Done()
			return nil, ctx.Err()
		}
		return dial(dialCtx, network, address)
	}
	result := QualifySvcRequestResult(ctx, r, http.StatusOK, fleetAttributionFactsReader{handlers: Handlers{}})
	if !result.Qualifies || dials.Load() != 1 {
		t.Fatalf("valid facts should finish on the existing connection: result=%#v dials=%d", result, dials.Load())
	}
}

func attributionConnectionFixture(t *testing.T) (*http.Transport, *atomic.Int32) {
	t.Helper()
	connections := new(atomic.Int32)
	server := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Impersonate-User") == "" || r.Header.Get("Impersonate-Group") == "" {
			t.Error("fact read omitted impersonation")
		}
		_, body := attributionLookupFixture(r.URL.Path)
		if body == "" {
			body = `{ "metadata": { "name": "pool-1" } }`
		}
		// A chunked response with a bounded trailing body must reach EOF before
		// the transport can reuse its connection for the next fact read.
		w.Header().Set("Content-Type", "application/json")
		w.(http.Flusher).Flush()
		if _, err := io.WriteString(w, body+strings.Repeat(" ", 16<<10)); err != nil {
			t.Errorf("response write: %v", err)
		}
	}))
	server.Config.ConnState = func(_ net.Conn, state http.ConnState) {
		if state == http.StateNew {
			connections.Add(1)
		}
	}
	server.Start()
	t.Cleanup(server.Close)
	transport := http.DefaultTransport.(*http.Transport).Clone()
	t.Cleanup(transport.CloseIdleConnections)
	oldClient, oldServer, oldToken := k8sClient, k8sAPIServer, k8sSAToken
	k8sClientOnce = sync.Once{}
	overrideK8sClient(&http.Client{Transport: transport}, server.URL, "synthetic-token")
	t.Cleanup(func() {
		k8sClientOnce = sync.Once{}
		k8sClient, k8sAPIServer, k8sSAToken = oldClient, oldServer, oldToken
	})
	return transport, connections
}

type attributionTrackedBody struct {
	io.Reader
	readBytes int
	closed    bool
}

func (body *attributionTrackedBody) Read(p []byte) (int, error) {
	n, err := body.Reader.Read(p)
	body.readBytes += n
	return n, err
}

func (body *attributionTrackedBody) Close() error {
	body.closed = true
	return nil
}

func TestCloseAttributionResponseBoundsDrainAndAlwaysCloses(t *testing.T) {
	for _, size := range []int{0, 16 << 10, 2 * k8sResponseBodyLimit} {
		body := &attributionTrackedBody{Reader: strings.NewReader(strings.Repeat(" ", size))}
		closeAttributionResponse(body)
		if !body.closed || body.readBytes != min(size, k8sResponseBodyLimit) {
			t.Fatalf("size=%d closed=%v drained=%d", size, body.closed, body.readBytes)
		}
	}
	body := &attributionTrackedBody{Reader: iotest.ErrReader(errors.New("synthetic read failure"))}
	closeAttributionResponse(body)
	if !body.closed {
		t.Fatal("read failure must not prevent closing the response")
	}
}

func TestCloseAttributionResponseRespectsRequestDeadline(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.(http.Flusher).Flush()
		// Leave the body incomplete until the client cancels the request.
		<-r.Context().Done()
	}))
	defer server.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 250*time.Millisecond)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, server.URL, nil)
	if err != nil {
		t.Fatal(err)
	}
	response, err := server.Client().Do(req)
	if err != nil {
		t.Fatal(err)
	}
	done := make(chan struct{})
	go func() {
		closeAttributionResponse(response.Body)
		close(done)
	}()
	select {
	case <-done:
		if !errors.Is(ctx.Err(), context.DeadlineExceeded) {
			t.Fatalf("unfinished body should be interrupted by the request deadline: %v", ctx.Err())
		}
	case <-time.After(2 * time.Second):
		t.Fatal("response cleanup outlived the request deadline")
	}
}
