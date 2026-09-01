// Package handlers — REST + reverse-proxy handlers for the cyclops-cs
// backend. Mirrors the constructor + utilities pattern from r33drichards/grt.
package handlers

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"regexp"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/chat"
	"cyclops-cs-backend/config"
	"cyclops-cs-backend/featureflagadmin"
	"cyclops-cs-backend/keycloak"
	"cyclops-cs-backend/productanalytics"
	"cyclops-cs-backend/signedurls"
	"cyclops-cs-backend/usage"

	"github.com/google/uuid"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/trace"
)

type UserAccountService interface {
	UserCreatedAt(ctx context.Context, subject string) (time.Time, error)
}

type SignedServiceURLService interface {
	Create(context.Context, signedurls.CreateInput) (signedurls.Record, error)
	List(context.Context, string, string) ([]signedurls.Record, error)
	Revoke(context.Context, string, uuid.UUID) (signedurls.Record, error)
}

type Handlers struct {
	Admin                    *keycloak.Admin
	GatewayCfg               config.GatewayConfiguration
	AuthCfg                  config.AuthConfiguration
	KC                       config.KeycloakConfiguration
	Stripe                   config.StripeConfiguration
	Billing                  BillingService
	UserAccounts             UserAccountService
	WebhookVerifier          WebhookVerifier
	Analytics                productanalytics.Capturer
	SignedServiceURLs        *signedurls.Service
	SignedServiceURLProvider func() *signedurls.Service
	signedServiceURLs        SignedServiceURLService
	signedServiceExists      func(context.Context, string, string, string) (bool, error)
	checkSignedServiceExists bool

	// Features carries the database-backed dependencies (the state query
	// executor and the GitHub trust policy store). It is a pointer because
	// setupRouter copies Handlers by value; see features.go.
	Features *Features
	Usage    usage.Provider

	adminAccessEvaluator  func(context.Context, *auth.User) (bool, error)
	usagePricingEvaluator func(context.Context, *auth.User) (auth.UsagePricing, error)

	FeatureFlags *featureflagadmin.Service

	Conversations       chat.ConversationStore
	Model               chat.ModelClient
	chatAccessEvaluator func(context.Context, *auth.User) (bool, error)
	chatLocks           *conversationLockRegistry

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
		Features:     NewFeatures(),
		Admin:        admin,
		UserAccounts: admin,
		GatewayCfg:   cfg.Gateway,
		AuthCfg:      cfg.Auth,
		KC:           cfg.Keycloak,
		Stripe:       cfg.Stripe,
		Analytics:    productanalytics.Nop(),
		chatLocks:    newConversationLockRegistry(),
	}
}

func (h Handlers) signedServiceURLService() SignedServiceURLService {
	if h.SignedServiceURLProvider != nil {
		if service := h.SignedServiceURLProvider(); service != nil {
			return service
		}
	}
	if h.SignedServiceURLs != nil {
		return h.SignedServiceURLs
	}
	return h.signedServiceURLs
}

func (h Handlers) usagePricing(ctx context.Context, user *auth.User) (auth.UsagePricing, error) {
	evaluator := h.usagePricingEvaluator
	if evaluator == nil {
		evaluator = auth.EvalUsagePricing
	}
	return evaluator(ctx, user)
}

func (h Handlers) isAdmin(ctx context.Context, user *auth.User) (bool, error) {
	evaluator := h.adminAccessEvaluator
	if evaluator == nil {
		evaluator = auth.EvalIsAdminFresh
	}
	return evaluator(ctx, user)
}

func (h Handlers) chatEnabled(ctx context.Context, user *auth.User) (bool, error) {
	if user == nil || user.ID == "" {
		return false, nil
	}
	evaluator := h.chatAccessEvaluator
	if evaluator == nil {
		evaluator = auth.EvalChatEnabled
	}
	return evaluator(ctx, user)
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

func isGitHubPrincipal(user *auth.User) bool {
	return user != nil && user.PrincipalType == auth.PrincipalTypeGitHubOIDC
}

func namespaceAllowed(user *auth.User, namespace string) bool {
	for _, allowed := range user.AllowedNamespaces {
		if allowed == namespace {
			return true
		}
	}
	return false
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
