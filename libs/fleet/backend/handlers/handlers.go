// Package handlers — REST + reverse-proxy handlers for the cyclops-cs
// backend. Mirrors the constructor + utilities pattern from r33drichards/grt.
package handlers

import (
	"bufio"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"regexp"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/config"
	"cyclops-cs-backend/keycloak"
	"cyclops-cs-backend/templates"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/trace"
)

type Handlers struct {
	Admin      *keycloak.Admin
	GatewayCfg config.GatewayConfiguration
	AuthCfg    config.AuthConfiguration
	KC         config.KeycloakConfiguration

	// Templates is the Redis-backed pool-template store. nil when no
	// Redis is configured — the pool-template handlers then reply 503.
	Templates templates.Store

	// WorkloadAdmin manages per-tenant clients in the workloads realm so
	// OSGym pool VMs can obtain a tenant-scoped OIDC token. nil disables
	// the feature (CreateNamespace then skips OIDC credential provisioning).
	// WorkloadAudience / WorkloadTokenURL are the aud claim and the token
	// endpoint baked into the per-tenant credentials Secret.
	WorkloadAdmin    *keycloak.Admin
	WorkloadAudience string
	WorkloadTokenURL string
}

func New(admin *keycloak.Admin, cfg *config.Configuration) Handlers {
	return Handlers{
		Admin:      admin,
		GatewayCfg: cfg.Gateway,
		AuthCfg:    cfg.Auth,
		KC:         cfg.Keycloak,
	}
}

var dnsLabel = regexp.MustCompile(`^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`)

func handlerTracer() trace.Tracer {
	return otel.Tracer("cyclops-cs-backend/handlers")
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func writeErr(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, map[string]string{"error": msg})
}

func currentUser(r *http.Request) *auth.User {
	return auth.GetUser(r.Context())
}

// statusCapture wraps http.ResponseWriter to capture the status code written
// by a reverse proxy for post-request metrics recording.
type statusCapture struct {
	http.ResponseWriter
	statusCode int
	written    bool
}

func (sc *statusCapture) WriteHeader(code int) {
	if !sc.written {
		sc.statusCode = code
		sc.written = true
	}
	sc.ResponseWriter.WriteHeader(code)
}

func (sc *statusCapture) Write(b []byte) (int, error) {
	if sc.statusCode == 0 {
		sc.statusCode = http.StatusOK
	}
	if !sc.written {
		sc.written = true
	}
	return sc.ResponseWriter.Write(b)
}

// Hijack forwards to the underlying ResponseWriter so httputil.ReverseProxy
// can switch protocols (WebSocket upgrades, e.g. noVNC's /websockify through
// /api/svc). Embedding only exposes the http.ResponseWriter interface methods,
// so without this explicit passthrough every 101 response fails with "can't
// switch protocols using non-Hijacker ResponseWriter".
func (sc *statusCapture) Hijack() (net.Conn, *bufio.ReadWriter, error) {
	hj, ok := sc.ResponseWriter.(http.Hijacker)
	if !ok {
		return nil, nil, fmt.Errorf("underlying ResponseWriter does not implement http.Hijacker")
	}
	if !sc.written {
		// A hijacked connection is a successful upgrade for metrics purposes.
		sc.statusCode = http.StatusSwitchingProtocols
		sc.written = true
	}
	return hj.Hijack()
}

// Flush forwards to the underlying ResponseWriter so streaming upstream
// responses (SSE, chunked) are not buffered by the wrapper.
func (sc *statusCapture) Flush() {
	if f, ok := sc.ResponseWriter.(http.Flusher); ok {
		f.Flush()
	}
}
