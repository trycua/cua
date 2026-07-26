package handlers

import (
	"context"
	"errors"
	"fmt"
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
			got := rewriteLocation(tc.location, tc.basePath)
			if got != tc.want {
				t.Errorf("rewriteLocation(%q, %q) = %q, want %q", tc.location, tc.basePath, got, tc.want)
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
