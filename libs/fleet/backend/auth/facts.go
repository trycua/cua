package auth

import (
	"context"
	"fmt"
	"net/http"
	"sync"
	"time"
)

// A FactProvider answers a question the token cannot: whether some other
// system — Kubernetes, a database, an API — says yes. The policy trees in
// policy_routes.go are pure data built before any of those systems has a
// client, which is the same problem RegisterPolicyModule solves for Rego text,
// and this file solves it the same way: a tree names a provider, and whatever
// owns the client registers one under that name at startup.
//
// The indirection is not only about construction order. handlers imports auth;
// auth cannot import handlers, and the namespace-RBAC probe is a handlers
// method. A registry is the seam that lets the policy depend on the probe
// without the packages depending on each other.

var factProviderRegistry = struct {
	sync.RWMutex
	providers map[string]FactProvider
}{providers: map[string]FactProvider{}}

// RegisterFactProvider binds a provider to a name that policy trees can refer
// to. Registering twice under one name replaces the earlier provider, which is
// what lets a test stand in for the production one — and what lets setupRouter
// be called more than once in a process.
func RegisterFactProvider(name string, provider FactProvider) {
	if name == "" || provider == nil {
		panic("registered fact provider requires a name and a provider")
	}
	factProviderRegistry.Lock()
	defer factProviderRegistry.Unlock()
	factProviderRegistry.providers[name] = provider
}

// RegisteredFacts returns a provider that resolves through the registry at
// request time. Nothing is looked up here: a tree is built during route
// construction, and the whole point is that the real provider need not exist
// yet.
func RegisteredFacts(name string) FactProvider {
	if name == "" {
		panic("registered fact provider requires a name")
	}
	return registeredFactProvider{name: name}
}

type registeredFactProvider struct {
	name string
}

// CacheKey answers from the name alone, without consulting the registry. It has
// to: the key is read while the plan is compiled — by Leaf.key for dedupe and by
// the cost model for ordering — and at that point the registry may still be
// empty. It also means the key is stable across a re-registration, which is
// correct, because what the key identifies is the load, not the loader.
func (provider registeredFactProvider) CacheKey() string {
	return "registered-facts:" + provider.name
}

// LoadFacts resolves the provider and delegates. An unregistered name is a
// plain error rather than a FactUnavailableError: nothing was unreachable, the
// process was assembled wrong, and the difference is a 500 that pages someone
// against a 502 that says "retry".
func (provider registeredFactProvider) LoadFacts(ctx context.Context, request *http.Request) (FactSet, error) {
	factProviderRegistry.RLock()
	delegate, ok := factProviderRegistry.providers[provider.name]
	factProviderRegistry.RUnlock()
	if !ok {
		return nil, fmt.Errorf("fact provider %q is not registered", provider.name)
	}
	return delegate.LoadFacts(ctx, request)
}

// The namespace-ownership fact: the name the probe is registered under, and the
// document it lands in. Both are consts rather than literals because three
// files have to agree on them — the tree in policy_routes.go, the provider in
// handlers, and authz_ownership.rego, which is the one that cannot be checked
// by the compiler.
const (
	// NamespaceRBACFactProvider is the registry name of the impersonated
	// RoleBinding probe. handlers registers it; NamespaceOwnershipPolicy names it.
	NamespaceRBACFactProvider = "namespace-rbac"
	// NamespaceRBACFactNamespace is where the probe's answer lands in the policy
	// input: authz_ownership.rego reads input.facts.namespace_rbac.allowed.
	NamespaceRBACFactNamespace = "namespace_rbac"
)

// Facts used by custom-resource creation admission.
const (
	StripeCardsFactProvider  = "stripe-cards"
	StripeCardsFactNamespace = "stripe_cards"
	CurrentYearFactProvider  = "current-year"
	CurrentMonthFactProvider = "current-month"
	TimeFactNamespace        = "time"
)

type clockFactProvider struct {
	cacheKey string
	key      string
	now      func() time.Time
}

type requestClockSnapshot struct {
	once sync.Once
	now  time.Time
}

type requestClockSnapshotKey struct{}

func (provider clockFactProvider) CacheKey() string { return provider.cacheKey }

func (provider clockFactProvider) LoadFacts(_ context.Context, request *http.Request) (FactSet, error) {
	var now time.Time
	if snapshot, ok := request.Context().Value(requestClockSnapshotKey{}).(*requestClockSnapshot); ok {
		snapshot.once.Do(func() { snapshot.now = provider.now().UTC() })
		now = snapshot.now
	} else {
		now = provider.now().UTC()
	}
	if provider.key == "current_year" {
		return FactSet{provider.key: now.Year()}, nil
	}
	return FactSet{provider.key: int(now.Month())}, nil
}

func CurrentYearFacts(now func() time.Time) FactProvider {
	if now == nil {
		panic("current-year fact provider requires a clock")
	}
	return clockFactProvider{cacheKey: CurrentYearFactProvider, key: "current_year", now: now}
}

func CurrentMonthFacts(now func() time.Time) FactProvider {
	if now == nil {
		panic("current-month fact provider requires a clock")
	}
	return clockFactProvider{cacheKey: CurrentMonthFactProvider, key: "current_month", now: now}
}
