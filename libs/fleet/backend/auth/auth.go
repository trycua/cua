// Package auth — Keycloak JWT validation, adapted from r33drichards/grt.
//
// Differences from upstream:
//   - Keycloak validation uses the configured issuer because service-account
//     tokens do not carry a useful `aud`; GitHub OIDC validates signed
//     audiences separately.
//   - OPA (Rego) is used for all route-level authorization via
//     OpaMiddleware; see auth/authz.rego.
//   - Three token families: exact interactive clients, per-key clients, and
//     user-key clients; route policy enforces each family and required claims.
package auth

import (
	"context"
	"fmt"
	"log"
	"log/slog"
	"net/http"
	"slices"
	"strings"
	"time"

	"cyclops-cs-backend/config"

	keyfunc "github.com/MicahParks/keyfunc/v2"
	jwt "github.com/golang-jwt/jwt/v5"
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
		AZP:           str(claims, "azp"),
		Namespace:     str(claims, "namespace"),
		PrincipalType: PrincipalTypeUser,
		Claims: map[string]string{
			"preferred_username": str(claims, "preferred_username"),
			"user_sub":           str(claims, "user_sub"),
			"user_groups":        str(claims, "user_groups"),
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
		return fmt.Errorf("missing or invalid github oidc audience")
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
