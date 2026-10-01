package handlers

import (
	"context"
	"sync/atomic"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/githubtrust"
)

// Features holds the database-backed dependencies that startup installs, in a
// form a background goroutine can replace while the server is running.
//
// Handlers cannot hold these as plain fields if they are ever to change after
// startup: setupRouter takes handlers.Handlers BY VALUE, and every route
// handler and fact provider it registers captures that copy. Assigning to a
// field afterwards updates nobody. Handlers therefore holds a *Features shared
// by every copy, and the dependencies inside are swapped atomically so a retry
// cannot race a request reading them.
//
// A nil *Features behaves like one with nothing installed, so Handlers{} stays
// usable in tests that do not exercise these routes.
type Features struct {
	stateQuery atomic.Pointer[stateQuerySlot]
	trustStore atomic.Pointer[trustStoreSlot]
}

// The slots exist because atomic.Pointer needs a concrete type. Storing the
// interfaces directly in an atomic.Value would panic the moment two different
// concrete implementations were stored, which is exactly what a retry does.
type stateQuerySlot struct{ executor StateQueryExecutor }

type trustStoreSlot struct{ store githubtrust.Store }

func NewFeatures() *Features {
	return &Features{}
}

// FeaturesWith is the constructor tests use to inject fakes; either argument
// may be nil.
func FeaturesWith(executor StateQueryExecutor, store githubtrust.Store) *Features {
	features := NewFeatures()
	features.SetStateQuery(executor)
	features.SetTrustStore(store)
	return features
}

func (f *Features) SetStateQuery(executor StateQueryExecutor) {
	if f == nil {
		return
	}
	f.stateQuery.Store(&stateQuerySlot{executor: executor})
}

func (f *Features) StateQuery() StateQueryExecutor {
	if f == nil {
		return nil
	}
	slot := f.stateQuery.Load()
	if slot == nil {
		return nil
	}
	return slot.executor
}

func (f *Features) SetTrustStore(store githubtrust.Store) {
	if f == nil {
		return
	}
	f.trustStore.Store(&trustStoreSlot{store: store})
}

func (f *Features) TrustStore() githubtrust.Store {
	if f == nil {
		return nil
	}
	slot := f.trustStore.Load()
	if slot == nil {
		return nil
	}
	return slot.store
}

// NewGitHubTrustResolverFor returns a resolver bound to the registry rather
// than to one store instance, so auth's resolver global is written exactly once
// at startup and never again. Rewriting that global from a retry goroutine
// would race every request reading it in auth.EvaluateGitHubTrust.
//
// The resolver is always non-nil; it reports the store's absence per request
// instead, which is what a not-yet-initialized database looks like.
//
// That changes one status code. Previously an uninitialized store left auth's
// resolver nil and EvaluateGitHubTrust failed with "github trust resolver not
// configured", which auth/middlewares.go answers with 401 "auth token is
// invalid". Returning ErrDatabaseUnavailable instead routes it through
// IsDatabaseUnavailable to 503 "authentication unavailable". 503 is the honest
// answer — the token was never judged, the database was simply missing — and it
// tells a GitHub OIDC client to retry, which now succeeds on its own once the
// retry loop installs the store. A 401 would fail a CI job permanently for a
// condition that clears itself.
func NewGitHubTrustResolverFor(features *Features) auth.GitHubTrustResolver {
	return registryTrustResolver{features: features}
}

type registryTrustResolver struct {
	features *Features
}

func (r registryTrustResolver) ResolveGitHubTrustPolicies(ctx context.Context, repository string) ([]auth.GitHubTrustPolicy, error) {
	store := r.features.TrustStore()
	if store == nil {
		return nil, auth.ErrDatabaseUnavailable
	}
	return githubTrustResolver{store: store}.ResolveGitHubTrustPolicies(ctx, repository)
}
