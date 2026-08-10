// Package auth — Keycloak JWT validation, adapted from r33drichards/grt.
//
// Differences from upstream:
//   - Issuer-based validation (`iss` must match the configured realm)
//     replaces grt's `aud`/scope checks — Keycloak service-account tokens
//     don't carry a useful `aud`, but `iss` + signature are sufficient
//     given we control the realm.
//   - OPA (Rego) is used for all route-level authorization via
//     OpaMiddleware; see auth/authz.rego.  The gateway surface additionally
//     enforces that the token's `namespace` claim equals "pool-{name}"
//     (CUA-527), preventing a compromised key from reaching any pool other
//     than its own.
//   - Three token families: exact interactive clients, per-key clients, and
//     user-key clients; route policy enforces each family and required claims.
package auth

import (
	"context"
	"fmt"
	"log"
	"log/slog"
	"net/http"
	"strings"
	"time"

	"cyclops-cs-backend/config"

	keyfunc "github.com/MicahParks/keyfunc/v2"
	jwt "github.com/golang-jwt/jwt/v5"
)

var (
	authConfig *config.AuthConfiguration
	cachedJWKS *keyfunc.JWKS
)

// Init mirrors grt's auth.Init — fetch JWKS once, keep refreshing.
func Init(c *config.AuthConfiguration) error {
	authConfig = c
	jwks, err := keyfunc.Get(c.JWKSUri, keyfunc.Options{
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
	cachedJWKS = jwks
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
	tok, err := jwt.Parse(raw, cachedJWKS.Keyfunc,
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
		ID:        str(claims, "sub"),
		Name:      str(claims, "name"),
		Email:     str(claims, "email"),
		AZP:       str(claims, "azp"),
		Namespace: str(claims, "namespace"),
		Claims: map[string]string{
			"preferred_username": str(claims, "preferred_username"),
			"user_sub":          str(claims, "user_sub"),
			"user_groups":       str(claims, "user_groups"),
		},
	}, nil
}

func str(c jwt.MapClaims, k string) string {
	if v, ok := c[k]; ok {
		if s, ok := v.(string); ok {
			return s
		}
	}
	return ""
}
