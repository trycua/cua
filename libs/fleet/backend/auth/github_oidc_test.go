package auth

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"cyclops-cs-backend/config"

	jwt "github.com/golang-jwt/jwt/v5"
)

const (
	testGitHubIssuer   = "https://token.actions.githubusercontent.com"
	testGitHubAudience = "fleets"
	testGitHubKeyID    = "github-test-key"
)

func TestValidate_GitHubOIDCResolvesOwnerAndNamespaces(t *testing.T) {
	signingKey, jwksURL := newGitHubJWKS(t)
	t.Cleanup(func() {
		SetGitHubTrustResolver(nil)
	})
	if err := Init(&config.AuthConfiguration{
		Issuer:             "https://issuer.example.test/realms/cyclops-cs",
		JWKSUri:            jwksURL,
		SigningAlgs:        []string{"RS256"},
		SPAClientID:        "cyclops-cs-spa",
		KeyClientPfx:       "key-",
		UserKeyClientPfx:   "ukey-",
		GitHubOIDCIssuer:   testGitHubIssuer,
		GitHubOIDCJWKSUri:  jwksURL,
		GitHubOIDCAudience: testGitHubAudience,
		GitHubOIDCEnabled:  true,
		GitHubOIDCAlgs:     []string{"RS256"},
	}); err != nil {
		t.Fatalf("Init err = %v", err)
	}
	SetGitHubTrustResolver(githubTrustResolverFunc(func(ctx context.Context, repository string) ([]GitHubTrustPolicy, error) {
		_ = ctx
		if repository != "trycua/cloud" {
			t.Fatalf("repository = %q", repository)
		}
		return []GitHubTrustPolicy{
			{ID: "p1", OwnerSub: "user-123", Repository: "trycua/cloud", AllowedNamespaces: []string{"ns-a"}, Enabled: true},
			{ID: "p2", OwnerSub: "user-123", Repository: "trycua/cloud", AllowedNamespaces: []string{"ns-b"}, Enabled: true},
		}, nil
	}))

	raw := signGitHubToken(t, signingKey, jwt.MapClaims{
		"iss":        testGitHubIssuer,
		"aud":        testGitHubAudience,
		"sub":        "repo:trycua/cloud:ref:refs/heads/main",
		"repository": "trycua/cloud",
		"exp":        time.Now().Add(time.Hour).Unix(),
		"iat":        time.Now().Add(-time.Minute).Unix(),
	})

	user, err := validate(raw)
	if err != nil {
		t.Fatalf("validate err = %v", err)
	}
	if user.PrincipalType != PrincipalTypeGitHubOIDC {
		t.Fatalf("principal_type = %q", user.PrincipalType)
	}
	if user.ID != "user-123" {
		t.Fatalf("owner_sub = %q", user.ID)
	}
	if got, want := user.AllowedNamespaces, []string{"ns-a", "ns-b"}; len(got) != len(want) || got[0] != want[0] || got[1] != want[1] {
		t.Fatalf("allowed_namespaces = %#v, want %#v", got, want)
	}
}

func TestValidate_GitHubOIDCAcceptsMigratedAudiences(t *testing.T) {
	signingKey, jwksURL := newGitHubJWKS(t)
	t.Cleanup(func() {
		SetGitHubTrustResolver(nil)
	})
	if err := Init(&config.AuthConfiguration{
		Issuer:                    "https://issuer.example.test/realms/cyclops-cs",
		JWKSUri:                   jwksURL,
		SigningAlgs:               []string{"RS256"},
		SPAClientID:               "cyclops-cs-spa",
		KeyClientPfx:              "key-",
		UserKeyClientPfx:          "ukey-",
		GitHubOIDCIssuer:          testGitHubIssuer,
		GitHubOIDCJWKSUri:         jwksURL,
		GitHubOIDCAudience:        testGitHubAudience,
		GitHubOIDCLegacyAudiences: []string{"cyclops-cs"},
		GitHubOIDCEnabled:         true,
		GitHubOIDCAlgs:            []string{"RS256"},
	}); err != nil {
		t.Fatalf("Init err = %v", err)
	}
	SetGitHubTrustResolver(githubTrustResolverFunc(func(_ context.Context, repository string) ([]GitHubTrustPolicy, error) {
		if repository != "trycua/cloud" {
			t.Fatalf("repository = %q", repository)
		}
		return []GitHubTrustPolicy{{
			ID:                "p1",
			OwnerSub:          "user-123",
			Repository:        "trycua/cloud",
			AllowedNamespaces: []string{"ns-a"},
			Enabled:           true,
		}}, nil
	}))

	for _, tt := range []struct {
		name     string
		audience string
		wantErr  string
	}{
		{name: "primary", audience: "fleets"},
		{name: "legacy", audience: "cyclops-cs"},
		{name: "rejected", audience: "unrelated-service", wantErr: "audience"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			raw := signGitHubToken(t, signingKey, jwt.MapClaims{
				"iss":        testGitHubIssuer,
				"aud":        tt.audience,
				"sub":        "repo:trycua/cloud:ref:refs/heads/main",
				"repository": "trycua/cloud",
				"exp":        time.Now().Add(time.Hour).Unix(),
				"iat":        time.Now().Add(-time.Minute).Unix(),
			})

			_, err := validate(raw)
			if tt.wantErr == "" {
				if err != nil {
					t.Fatalf("validate err = %v", err)
				}
				return
			}
			if err == nil || !strings.Contains(err.Error(), tt.wantErr) {
				t.Fatalf("validate err = %v, want error containing %q", err, tt.wantErr)
			}
		})
	}
}

func TestValidate_GitHubOIDCFailsOnCrossOwnerCollision(t *testing.T) {
	signingKey, jwksURL := newGitHubJWKS(t)
	t.Cleanup(func() {
		SetGitHubTrustResolver(nil)
	})
	if err := Init(&config.AuthConfiguration{
		Issuer:             "https://issuer.example.test/realms/cyclops-cs",
		JWKSUri:            jwksURL,
		SigningAlgs:        []string{"RS256"},
		SPAClientID:        "cyclops-cs-spa",
		KeyClientPfx:       "key-",
		UserKeyClientPfx:   "ukey-",
		GitHubOIDCIssuer:   testGitHubIssuer,
		GitHubOIDCJWKSUri:  jwksURL,
		GitHubOIDCAudience: testGitHubAudience,
		GitHubOIDCEnabled:  true,
		GitHubOIDCAlgs:     []string{"RS256"},
	}); err != nil {
		t.Fatalf("Init err = %v", err)
	}
	SetGitHubTrustResolver(githubTrustResolverFunc(func(ctx context.Context, repository string) ([]GitHubTrustPolicy, error) {
		_ = ctx
		_ = repository
		return []GitHubTrustPolicy{
			{ID: "p1", OwnerSub: "user-123", Repository: "trycua/cloud", AllowedNamespaces: []string{"ns-a"}, Enabled: true},
			{ID: "p2", OwnerSub: "user-456", Repository: "trycua/cloud", AllowedNamespaces: []string{"ns-b"}, Enabled: true},
		}, nil
	}))

	raw := signGitHubToken(t, signingKey, jwt.MapClaims{
		"iss":        testGitHubIssuer,
		"aud":        testGitHubAudience,
		"sub":        "repo:trycua/cloud:ref:refs/heads/main",
		"repository": "trycua/cloud",
		"exp":        time.Now().Add(time.Hour).Unix(),
		"iat":        time.Now().Add(-time.Minute).Unix(),
	})

	if _, err := validate(raw); err == nil {
		t.Fatal("expected cross-owner collision to fail")
	}
}

type githubTrustResolverFunc func(context.Context, string) ([]GitHubTrustPolicy, error)

func (fn githubTrustResolverFunc) ResolveGitHubTrustPolicies(ctx context.Context, repository string) ([]GitHubTrustPolicy, error) {
	return fn(ctx, repository)
}

func newGitHubJWKS(t *testing.T) (*rsa.PrivateKey, string) {
	t.Helper()
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatalf("GenerateKey: %v", err)
	}
	jwksBody, err := json.Marshal(map[string]any{
		"keys": []map[string]any{{
			"kty": "RSA",
			"use": "sig",
			"alg": "RS256",
			"kid": testGitHubKeyID,
			"n":   base64.RawURLEncoding.EncodeToString(key.N.Bytes()),
			"e":   base64.RawURLEncoding.EncodeToString(bigEndianBytes(key.E)),
		}},
	})
	if err != nil {
		t.Fatalf("marshal jwks: %v", err)
	}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write(jwksBody)
	}))
	t.Cleanup(server.Close)
	return key, server.URL
}

func signGitHubToken(t *testing.T, key *rsa.PrivateKey, claims jwt.MapClaims) string {
	t.Helper()
	token := jwt.NewWithClaims(jwt.SigningMethodRS256, claims)
	token.Header["kid"] = testGitHubKeyID
	raw, err := token.SignedString(key)
	if err != nil {
		t.Fatalf("SignedString: %v", err)
	}
	return raw
}

func bigEndianBytes(v int) []byte {
	if v == 0 {
		return []byte{0}
	}
	var out []byte
	for n := v; n > 0; n >>= 8 {
		out = append([]byte{byte(n & 0xff)}, out...)
	}
	return out
}
