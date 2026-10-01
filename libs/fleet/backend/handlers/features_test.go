package handlers

import (
	"context"
	"errors"
	"testing"

	"cyclops-cs-backend/auth"
)

// The trust resolver is installed into an unguarded package global in auth that
// every request reads, so it is bound to the registry once and never rewritten.
// It therefore has to pick up a store installed after the fact.
func TestTrustResolverSeesAStoreInstalledLater(t *testing.T) {
	features := NewFeatures()
	resolver := NewGitHubTrustResolverFor(features)

	if _, err := resolver.ResolveGitHubTrustPolicies(context.Background(), "trycua/cloud"); !errors.Is(err, auth.ErrDatabaseUnavailable) {
		t.Fatalf("error with no store = %v, want ErrDatabaseUnavailable", err)
	}

	features.SetTrustStore(&fakeGitHubTrustStore{})

	if _, err := resolver.ResolveGitHubTrustPolicies(context.Background(), "trycua/cloud"); errors.Is(err, auth.ErrDatabaseUnavailable) {
		t.Fatal("resolver still reports the database unavailable after a store was installed")
	}
}

func TestFeaturesAreNilSafe(t *testing.T) {
	var features *Features
	if features.StateQuery() != nil || features.TrustStore() != nil {
		t.Fatal("nil registry should read as empty")
	}
	features.SetStateQuery(nil) // must not panic
}

// An uninitialized store must read as "database unavailable", not as a generic
// failure: auth/middlewares.go turns the former into 503 and everything else
// into 401, and a GitHub OIDC client hitting a pod whose database has not come
// up yet should be told to retry rather than that its token is invalid. The
// retry loop makes that retry succeed on its own.
func TestTrustResolverReportsAnAbsentStoreAsDatabaseUnavailable(t *testing.T) {
	resolver := NewGitHubTrustResolverFor(NewFeatures())

	_, err := resolver.ResolveGitHubTrustPolicies(context.Background(), "trycua/cloud")

	if !auth.IsDatabaseUnavailable(err) {
		t.Fatalf("IsDatabaseUnavailable(%v) = false, want true so the caller answers 503 rather than 401", err)
	}
}
