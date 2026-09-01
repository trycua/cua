// Package auth — Keycloak JWT validation and route authorization, adapted from
// r33drichards/grt.
//
// Authentication. Keycloak validation uses the configured issuer because
// service-account tokens do not carry a useful `aud`; GitHub OIDC validates
// signed audiences separately. Three token families reach the policy — exact
// interactive clients, per-key clients, and user-key clients — plus principals
// whose azp cannot be enumerated (oauth2-proxy sessions, RFC 7591 dynamically
// registered MCP clients). authz.rego names the three families; a surface that
// accepts the fourth kind says so by not requiring a family.
//
// Authorization is OPA (Rego), composed rather than monolithic. Every
// authenticated route runs
//
//	All(BasePolicy(), <its surface>)
//
// where the base is a non-empty subject — the one condition every rule of the
// former single-module policy shared — and the surface is one module per group
// of routes: authz_keys.rego, authz_k8s.rego, authz_svc.rego, and so on.
// /api/k8s adds pool_admission.rego as a third conjunct over the request body.
//
// Two conjuncts read something beyond the token. pool_admission.rego reads the
// request body; authz_ownership.rego — the namespace-tenancy boundary on
// /api/svc and GET /api/namespaces/{name} — reads a Kubernetes RBAC
// probe, delivered as input.facts by a FactProvider that handlers registers and
// main.go binds. Both are separate leaves rather than rules folded into a
// surface, so the expensive read only happens on the requests a cheap sibling
// has not already decided.
//
// Which surface a route runs is decided in exactly one place, the routeSurfaces
// table in policy_routes.go, which main.go reads through RouteMiddleware. That
// is what keeps the routes and the policy from drifting apart: a registered
// route the table does not name cannot start, and a surface naming routes that
// no longer exist fails route_policy_correspondence_test.go rather than sitting
// there looking like protection. Compiled plans are memoized per surface, so
// routes sharing a surface share one plan.
package auth

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log"
	"log/slog"
	"net"
	"net/http"
	"slices"
	"strings"
	"time"

	"cyclops-cs-backend/config"

	keyfunc "github.com/MicahParks/keyfunc/v2"
	jwt "github.com/golang-jwt/jwt/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

var (
	authConfig         *config.AuthConfiguration
	cachedKeycloakJWKS *keyfunc.JWKS
	cachedGitHubJWKS   *keyfunc.JWKS
)

type GitHubTrustPolicy struct {
	ID                string
	OwnerSub          string
	Repository        string
	AllowedNamespaces []string
	Enabled           bool
}

type GitHubTrustResolver interface {
	ResolveGitHubTrustPolicies(ctx context.Context, repository string) ([]GitHubTrustPolicy, error)
}

var ErrDatabaseUnavailable = errors.New("database unavailable")

type databaseContextKey struct{}

const DatabaseRequestTimeout = 15 * time.Second

func DatabaseContext(parent context.Context) (context.Context, context.CancelFunc) {
	if databaseContext, ok := parent.Value(databaseContextKey{}).(context.Context); ok {
		return databaseContext, func() {}
	}
	if deadline, ok := parent.Deadline(); ok && time.Until(deadline) <= DatabaseRequestTimeout {
		return context.WithValue(parent, databaseContextKey{}, parent), func() {}
	}
	databaseContext, cancel := context.WithTimeout(parent, DatabaseRequestTimeout)
	return context.WithValue(databaseContext, databaseContextKey{}, databaseContext), cancel
}

func IsDatabaseUnavailable(err error) bool {
	return errors.Is(err, ErrDatabaseUnavailable) || errors.Is(err, context.DeadlineExceeded) || errors.Is(err, context.Canceled)
}

func DatabaseUnavailable(err error) error {
	if errors.Is(err, ErrDatabaseUnavailable) {
		return err
	}
	return errors.Join(ErrDatabaseUnavailable, err)
}

func ClassifyDatabaseError(err error) error {
	if err == nil || errors.Is(err, ErrDatabaseUnavailable) {
		return err
	}
	if errors.Is(err, context.DeadlineExceeded) || errors.Is(err, context.Canceled) {
		return errors.Join(DatabaseUnavailable(err), err)

	}

	var postgresError *pgconn.PgError
	if errors.As(err, &postgresError) {
		return err
	}
	var connectError *pgconn.ConnectError
	if errors.As(err, &connectError) {
		return errors.Join(DatabaseUnavailable(err), err)

	}
	var networkError net.Error
	if errors.As(err, &networkError) || errors.Is(err, net.ErrClosed) || errors.Is(err, io.EOF) || errors.Is(err, io.ErrUnexpectedEOF) {
		return errors.Join(DatabaseUnavailable(err), err)

	}
	return err
}

var githubTrustResolver GitHubTrustResolver

func SetGitHubTrustResolver(resolver GitHubTrustResolver) {
	githubTrustResolver = resolver
}

// Init mirrors grt's auth.Init — fetch JWKS once, keep refreshing.
func Init(c *config.AuthConfiguration) error {
	authConfig = c
	keycloakJWKS, err := keyfunc.Get(c.JWKSUri, keyfunc.Options{
		RequestFactory: func(ctx context.Context, url string) (*http.Request, error) {
			req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
			if err != nil {
				return nil, err
			}
			return req, nil
		},
		Client:            &http.Client{Timeout: 15 * time.Second},
		RefreshInterval:   time.Hour,
		RefreshTimeout:    10 * time.Second,
		RefreshUnknownKID: true,
		RefreshErrorHandler: func(err error) {
			slog.Error("JWKS refresh", "error", err.Error())
		},
	})
	if err != nil {
		return fmt.Errorf("fetch JWKS at %s: %w", c.JWKSUri, err)
	}
	cachedKeycloakJWKS = keycloakJWKS
	cachedGitHubJWKS = nil
	if c.GitHubOIDCEnabled && c.GitHubOIDCJWKSUri != "" {
		githubJWKS, err := keyfunc.Get(c.GitHubOIDCJWKSUri, keyfunc.Options{
			RequestFactory: func(ctx context.Context, url string) (*http.Request, error) {
				req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
				if err != nil {
					return nil, err
				}
				return req, nil
			},
			Client:            &http.Client{Timeout: 15 * time.Second},
			RefreshInterval:   time.Hour,
			RefreshTimeout:    10 * time.Second,
			RefreshUnknownKID: true,
			RefreshErrorHandler: func(err error) {
				slog.Error("GitHub JWKS refresh", "error", err.Error())
			},
		})
		if err != nil {
			return fmt.Errorf("fetch GitHub JWKS at %s: %w", c.GitHubOIDCJWKSUri, err)
		}
		cachedGitHubJWKS = githubJWKS
	}
	return nil
}

// MustInit panics on Init failure — for use inside main().
func MustInit(c *config.AuthConfiguration) {
	if err := Init(c); err != nil {
		log.Fatalf("auth init: %v", err)
	}
}

func extractToken(r *http.Request) (string, error) {
	h := r.Header.Get("Authorization")
	parts := strings.SplitN(h, " ", 2)
	if len(parts) != 2 || !strings.EqualFold(parts[0], "Bearer") || parts[1] == "" {
		return "", fmt.Errorf("missing or malformed Authorization header")
	}
	return parts[1], nil
}

func sliceContains[K comparable](s []K, e K) bool {
	for _, a := range s {
		if a == e {
			return true
		}
	}
	return false
}

// validate parses + verifies the token signature, expiry, issuer, and
// signing algorithm. It does NOT enforce `azp` / `namespace` — those are
// up to the calling middleware.
func validate(raw string) (*User, error) {
	return validateWithContext(context.Background(), raw)
}

func validateWithContext(ctx context.Context, raw string) (*User, error) {
	claims, err := parseUnverifiedClaims(raw)
	if err != nil {
		return nil, err
	}
	switch iss := str(claims, "iss"); {
	case iss == authConfig.Issuer:
		return validateKeycloak(raw)
	case authConfig != nil && authConfig.GitHubOIDCEnabled && iss == authConfig.GitHubOIDCIssuer:
		return validateGitHub(ctx, raw)
	default:
		return nil, fmt.Errorf("unsupported token issuer")
	}
}

func validateKeycloak(raw string) (*User, error) {
	tok, err := jwt.Parse(raw, cachedKeycloakJWKS.Keyfunc,
		jwt.WithLeeway(30*time.Second),
		jwt.WithIssuer(authConfig.Issuer),
		jwt.WithValidMethods(authConfig.SigningAlgs),
	)
	if err != nil {
		return nil, err
	}
	if !tok.Valid {
		return nil, fmt.Errorf("invalid token")
	}
	if !sliceContains(authConfig.SigningAlgs, tok.Method.Alg()) {
		return nil, fmt.Errorf("disallowed signing alg %q", tok.Method.Alg())
	}
	claims, ok := tok.Claims.(jwt.MapClaims)
	if !ok {
		return nil, fmt.Errorf("unexpected claims type")
	}
	return &User{
		ID:            str(claims, "sub"),
		Name:          str(claims, "name"),
		Email:         str(claims, "email"),
		EmailVerified: boolean(claims, "email_verified"),
		AZP:           str(claims, "azp"),
		Namespace:     str(claims, "namespace"),
		PrincipalType: PrincipalTypeUser,
		Claims: map[string]string{
			"preferred_username": str(claims, "preferred_username"),
			"user_sub":           str(claims, "user_sub"),
			"user_groups":        str(claims, "user_groups"),
			"user_email":         str(claims, "user_email"),
			"user_email_verified": fmt.Sprint(
				boolean(claims, "user_email_verified"),
			),
		},
	}, nil
}

func validateGitHub(ctx context.Context, raw string) (*User, error) {
	if cachedGitHubJWKS == nil {
		return nil, fmt.Errorf("github oidc disabled")
	}
	tok, err := jwt.Parse(raw, cachedGitHubJWKS.Keyfunc,
		jwt.WithLeeway(30*time.Second),
		jwt.WithIssuer(authConfig.GitHubOIDCIssuer),
		jwt.WithValidMethods(authConfig.GitHubOIDCAlgs),
	)
	if err != nil {
		return nil, err
	}
	if !tok.Valid {
		return nil, fmt.Errorf("invalid token")
	}
	claims, ok := tok.Claims.(jwt.MapClaims)
	if !ok {
		return nil, fmt.Errorf("unexpected claims type")
	}
	if err := validateGitHubAudience(
		claims,
		authConfig.GitHubOIDCAudience,
		authConfig.GitHubOIDCLegacyAudiences,
	); err != nil {
		return nil, err
	}
	repository := str(claims, "repository")
	if repository == "" {
		return nil, fmt.Errorf("missing repository claim")
	}
	if githubTrustResolver == nil {
		return nil, fmt.Errorf("github trust resolver not configured")
	}
	policies, err := githubTrustResolver.ResolveGitHubTrustPolicies(ctx, repository)
	if err != nil {
		return nil, err
	}
	ownerSub := ""
	allowedSet := map[string]struct{}{}
	policyIDs := make([]string, 0, len(policies))
	for _, policy := range policies {
		if !policy.Enabled || policy.Repository != repository {
			continue
		}
		if ownerSub == "" {
			ownerSub = policy.OwnerSub
		}
		if ownerSub != policy.OwnerSub {
			return nil, fmt.Errorf("conflicting github trust policies")
		}
		policyIDs = append(policyIDs, policy.ID)
		for _, ns := range policy.AllowedNamespaces {
			allowedSet[ns] = struct{}{}
		}
	}
	if ownerSub == "" {
		return nil, fmt.Errorf("no matching github trust policy")
	}
	allowedNamespaces := make([]string, 0, len(allowedSet))
	for ns := range allowedSet {
		allowedNamespaces = append(allowedNamespaces, ns)
	}
	slices.Sort(allowedNamespaces)
	slices.Sort(policyIDs)
	return &User{
		ID:                ownerSub,
		AZP:               "github-oidc",
		PrincipalType:     PrincipalTypeGitHubOIDC,
		Repository:        repository,
		AllowedNamespaces: allowedNamespaces,
		PolicyIDs:         policyIDs,
		Claims: map[string]string{
			"github_sub": str(claims, "sub"),
		},
	}, nil
}

func validateGitHubAudience(claims jwt.MapClaims, primary string, legacy []string) error {
	audiences, err := claims.GetAudience()
	if err != nil || len(audiences) == 0 {
		return errors.Join(fmt.Errorf("missing or invalid github oidc audience"), err)

	}
	accepted := append([]string{primary}, legacy...)
	for _, audience := range audiences {
		if slices.Contains(accepted, audience) {
			return nil
		}
	}
	return fmt.Errorf("github oidc audience is not accepted")
}

func parseUnverifiedClaims(raw string) (jwt.MapClaims, error) {
	parser := jwt.NewParser()
	claims := jwt.MapClaims{}
	if _, _, err := parser.ParseUnverified(raw, claims); err != nil {
		return nil, err
	}
	return claims, nil
}

func str(c jwt.MapClaims, k string) string {
	if v, ok := c[k]; ok {
		if s, ok := v.(string); ok {
			return s
		}
	}
	return ""
}

func boolean(c jwt.MapClaims, k string) bool {
	v, ok := c[k]
	if !ok {
		return false
	}
	value, ok := v.(bool)
	return ok && value
}
