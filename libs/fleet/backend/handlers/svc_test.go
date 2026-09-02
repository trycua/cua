package handlers

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/http/httputil"
	"net/url"
	"sync"
	"testing"
	"time"

	"cyclops-cs-backend/config"
)

func TestRewriteLocation(t *testing.T) {
	tests := []struct {
		name     string
		location string
		basePath string
		want     string
	}{
		{"absolute /llms.txt", "/llms.txt", "/api/svc/myns/mysvc", "/api/svc/myns/mysvc/llms.txt"},
		{"absolute /foo/bar", "/foo/bar", "/api/svc/pool-test/test-abc-mcp", "/api/svc/pool-test/test-abc-mcp/foo/bar"},
		{"root /", "/", "/api/svc/ns/svc", "/api/svc/ns/svc/"},
		{"relative path", "other.html", "/api/svc/ns/svc", "/api/svc/ns/svc/other.html"},
		{"external URL unchanged", "https://example.com/foo", "/api/svc/ns/svc", "https://example.com/foo"},
		{"empty unchanged", "", "/api/svc/ns/svc", ""},
		{"absolute with query", "/search?q=test", "/api/svc/ns/svc", "/api/svc/ns/svc/search?q=test"},
		{"absolute with fragment", "/page#section", "/api/svc/ns/svc", "/api/svc/ns/svc/page#section"},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := rewriteLocation(tc.location, tc.basePath, "/")
			if got != tc.want {
				t.Errorf("rewriteLocation(%q, %q) = %q, want %q", tc.location, tc.basePath, got, tc.want)
			}
		})
	}
}

func TestNormalizeProxyUpstreamPath(t *testing.T) {
	tests := []struct {
		name string
		in   string
		want string
	}{
		{name: "empty", in: "", want: "/"},
		{name: "root slash", in: "/", want: "/"},
		{name: "relative", in: "tools", want: "/tools"},
		{name: "absolute", in: "/tools", want: "/tools"},
		{name: "double slash", in: "//tools", want: "/tools"},
		{name: "slash backslash", in: "/\\tools", want: "/tools"},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if got := normalizeProxyUpstreamPath(tc.in); got != tc.want {
				t.Fatalf("normalizeProxyUpstreamPath(%q)=%q want %q", tc.in, got, tc.want)
			}
		})
	}
}

func TestSvc_Unauthenticated(t *testing.T) {
	h := Handlers{
		GatewayCfg: config.GatewayConfiguration{
			Scheme:        "http",
			Port:          "80",
			ClusterDomain: "svc.cluster.local",
		},
	}

	w := httptest.NewRecorder()
	r := httptest.NewRequest("GET", "/api/svc/ns/svc/path", nil)
	r.SetPathValue("namespace", "ns")
	r.SetPathValue("service", "svc")
	r.SetPathValue("path", "path")

	h.Svc(w, r)

	if w.Code != http.StatusUnauthorized {
		t.Errorf("expected 401, got %d", w.Code)
	}
}

func TestSvcReverseProxyPreservesLargeBodies(t *testing.T) {
	payload := bytes.Repeat([]byte{0, 1, 2, 255}, 300*1024)
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, err := io.ReadAll(r.Body)
		if err != nil {
			t.Errorf("read request: %v", err)
			return
		}
		if !bytes.Equal(body, payload) {
			t.Errorf("request changed: got %d want %d", len(body), len(payload))
		}
		_, _ = w.Write(body)
	}))
	defer upstream.Close()
	target, err := url.Parse(upstream.URL)
	if err != nil {
		t.Fatalf("parse upstream URL: %v", err)
	}
	req := httptest.NewRequest(http.MethodPost, "/upload", bytes.NewReader(payload))
	res := httptest.NewRecorder()
	newSvcReverseProxy(target, "/upload", "/api/svc/ns/svc").ServeHTTP(res, req)
	if res.Code != http.StatusOK {
		t.Fatalf("status=%d body=%s", res.Code, res.Body.String())
	}
	if !bytes.Equal(res.Body.Bytes(), payload) {
		t.Fatalf("response changed: got %d want %d", res.Body.Len(), len(payload))
	}
}

func TestSvcReverseProxyStreamsRequestWithBackpressure(t *testing.T) {
	prefixRead := make(chan struct{})
	release := make(chan struct{})
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		prefix := make([]byte, 5)
		if _, err := io.ReadFull(r.Body, prefix); err != nil {
			t.Errorf("read prefix: %v", err)
			return
		}
		if string(prefix) != "first" {
			t.Errorf("prefix=%q", prefix)
		}
		close(prefixRead)
		<-release
		rest, err := io.ReadAll(r.Body)
		if err != nil {
			t.Errorf("read rest: %v", err)
			return
		}
		_, _ = w.Write(rest)
	}))
	defer upstream.Close()
	target, err := url.Parse(upstream.URL)
	if err != nil {
		t.Fatalf("parse upstream URL: %v", err)
	}
	proxy := httptest.NewServer(newSvcReverseProxy(target, "/upload", "/api/svc/ns/svc"))
	defer proxy.Close()
	reader, writer := io.Pipe()
	req, err := http.NewRequest(http.MethodPost, proxy.URL, reader)
	if err != nil {
		t.Fatalf("build request: %v", err)
	}
	result := make(chan struct {
		resp *http.Response
		err  error
	}, 1)
	go func() {
		resp, err := http.DefaultClient.Do(req)
		result <- struct {
			resp *http.Response
			err  error
		}{resp, err}
	}()
	if _, err := writer.Write([]byte("first")); err != nil {
		t.Fatal(err)
	}
	select {
	case <-prefixRead:
	case <-time.After(time.Second):
		t.Fatal("request prefix was buffered")
	}
	close(release)
	if _, err := writer.Write([]byte("second")); err != nil {
		t.Fatal(err)
	}
	_ = writer.Close()
	select {
	case got := <-result:
		if got.err != nil {
			t.Fatal(got.err)
		}
		defer got.resp.Body.Close()
		body, err := io.ReadAll(got.resp.Body)
		if err != nil {
			t.Fatalf("read response: %v", err)
		}
		if string(body) != "second" {
			t.Fatalf("body=%q", body)
		}
	case <-time.After(time.Second):
		t.Fatal("proxy response timed out")
	}
}

func TestSvcReverseProxyFlushesStreamingResponse(t *testing.T) {
	release := make(chan struct{})
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte("first\n"))
		w.(http.Flusher).Flush()
		<-release
		_, _ = w.Write([]byte("second\n"))
	}))
	defer upstream.Close()
	target, err := url.Parse(upstream.URL)
	if err != nil {
		t.Fatalf("parse upstream URL: %v", err)
	}
	proxy := httptest.NewServer(newSvcReverseProxy(target, "/stream", "/api/svc/ns/svc"))
	defer proxy.Close()
	resp, err := http.Get(proxy.URL)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	first := make([]byte, 6)
	read := make(chan error, 1)
	go func() { _, err := io.ReadFull(resp.Body, first); read <- err }()
	select {
	case err := <-read:
		if err != nil {
			t.Fatal(err)
		}
		if string(first) != "first\n" {
			t.Fatalf("first=%q", first)
		}
	case <-time.After(time.Second):
		t.Fatal("first chunk not flushed")
	}
	close(release)
	rest, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("read response remainder: %v", err)
	}
	if string(rest) != "second\n" {
		t.Fatalf("rest=%q", rest)
	}
}

func TestSvcProxyErrorStatus(t *testing.T) {
	tests := []struct {
		name string
		err  error
		want int
	}{
		{name: "client canceled", err: context.Canceled, want: 499},
		{name: "wrapped client canceled", err: errors.Join(errors.New("proxy"), context.Canceled), want: 499},
		{name: "other proxy error", err: errors.New("dial tcp: no route to host"), want: http.StatusBadGateway},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if got := svcProxyErrorStatus(tc.err); got != tc.want {
				t.Fatalf("svcProxyErrorStatus(%v) = %d, want %d", tc.err, got, tc.want)
			}
		})
	}
}

func TestSvcProxyErrorStatus_FromReverseProxyClientDisconnect(t *testing.T) {
	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(250 * time.Millisecond)
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("ok"))
	}))
	defer backend.Close()

	target, err := url.Parse(backend.URL)
	if err != nil {
		t.Fatalf("parse backend URL: %v", err)
	}

	errCh := make(chan error, 1)
	proxy := httputil.NewSingleHostReverseProxy(target)
	proxy.ErrorHandler = func(rw http.ResponseWriter, req *http.Request, err error) {
		errCh <- err
		rw.WriteHeader(svcProxyErrorStatus(err))
	}

	srv := &http.Server{Handler: proxy}
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	defer ln.Close()

	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		_ = srv.Serve(ln)
	}()
	defer func() {
		_ = srv.Close()
		wg.Wait()
	}()

	conn, err := net.Dial("tcp", ln.Addr().String())
	if err != nil {
		t.Fatalf("dial proxy: %v", err)
	}
	_, _ = fmt.Fprintf(conn, "GET / HTTP/1.1\r\nHost: %s\r\n\r\n", ln.Addr().String())
	time.Sleep(50 * time.Millisecond)
	_ = conn.Close()

	select {
	case err := <-errCh:
		if got := svcProxyErrorStatus(err); got != statusClientClosedRequest {
			t.Fatalf("svcProxyErrorStatus(%v) = %d, want %d", err, got, statusClientClosedRequest)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("timed out waiting for reverse proxy error")
	}
}

func TestSvcProxyErrorStatus_FromReverseProxyDeadlineExceeded(t *testing.T) {
	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(250 * time.Millisecond)
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("ok"))
	}))
	defer backend.Close()

	target, err := url.Parse(backend.URL)
	if err != nil {
		t.Fatalf("parse backend URL: %v", err)
	}

	errCh := make(chan error, 1)
	proxy := httputil.NewSingleHostReverseProxy(target)
	proxy.ErrorHandler = func(rw http.ResponseWriter, req *http.Request, err error) {
		errCh <- err
		rw.WriteHeader(svcProxyErrorStatus(err))
	}

	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		ctx, cancel := context.WithTimeout(r.Context(), 50*time.Millisecond)
		defer cancel()
		proxy.ServeHTTP(w, r.WithContext(ctx))
	})

	srv := httptest.NewServer(handler)
	defer srv.Close()

	resp, err := http.Get(srv.URL)
	if err != nil {
		t.Fatalf("GET %s: %v", srv.URL, err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadGateway {
		t.Fatalf("response status = %d, want %d", resp.StatusCode, http.StatusBadGateway)
	}

	select {
	case err := <-errCh:
		if !errors.Is(err, context.DeadlineExceeded) {
			t.Fatalf("expected context deadline exceeded, got %v", err)
		}
		if got := svcProxyErrorStatus(err); got != http.StatusBadGateway {
			t.Fatalf("svcProxyErrorStatus(%v) = %d, want %d", err, got, http.StatusBadGateway)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("timed out waiting for reverse proxy error")
	}
}
