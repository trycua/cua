package auth

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"

	"github.com/open-policy-agent/opa/ast"
	"github.com/open-policy-agent/opa/dependencies"
	"github.com/open-policy-agent/opa/rego"
)

// The namespace-ownership conjunct is the first policy in this repo that needs
// something no token carries, so it is the first one whose cost is not a
// constant. What it costs is the number of times its FactProvider is asked, and
// that number is what these tests measure — not because a slow authorization
// stage is untidy, but because /api/svc serves every noVNC asset, and a probe
// per asset request is an apiserver incident.
//
// Everything here is measured against the production trees from
// policy_routes.go, through Compile and the real fold. A count taken against a
// tree written out in this file would only say what this file does.

// countingFacts records how many times it was asked, and answers from the
// namespace so a test can pick allow, deny, or unavailable by choosing one.
type countingFacts struct {
	loads       atomic.Int64
	unreachable string
	allowed     string
}

func (facts *countingFacts) CacheKey() string { return NamespaceRBACFactProvider }

func (facts *countingFacts) LoadFacts(_ context.Context, request *http.Request) (FactSet, error) {
	facts.loads.Add(1)
	namespace := OwnedNamespace(request.Context())
	if facts.unreachable != "" && namespace == facts.unreachable {
		return nil, &FactUnavailableError{Namespace: NamespaceRBACFactNamespace, Err: errors.New("apiserver unavailable")}
	}
	return FactSet{"allowed": namespace == facts.allowed}, nil
}

// installCountingFacts registers a counting provider for the duration of a test
// and restores whatever was there before, so tests in this package that share
// the registry do not depend on running order.
func installCountingFacts(t *testing.T, allowed string) *countingFacts {
	t.Helper()
	LoadOpa()
	facts := &countingFacts{allowed: allowed}
	factProviderRegistry.RLock()
	previous, existed := factProviderRegistry.providers[NamespaceRBACFactProvider]
	factProviderRegistry.RUnlock()
	RegisterFactProvider(NamespaceRBACFactProvider, facts)
	t.Cleanup(func() {
		if existed {
			RegisterFactProvider(NamespaceRBACFactProvider, previous)
			return
		}
		factProviderRegistry.Lock()
		delete(factProviderRegistry.providers, NamespaceRBACFactProvider)
		factProviderRegistry.Unlock()
	})
	return facts
}

func svcCase(namespace string) routeCase {
	return routeCase{
		name:   namespace,
		params: map[string]string{"namespace": namespace, "service": "svc-a", "path": "v1/status"},
		path:   "/api/svc/" + namespace + "/svc-a/v1/status",
	}
}

const svcPathRoute = "/api/svc/{namespace}/{service}/{path...}"

// evalSurface runs a route's production plan over one request, exactly as
// PolicyMiddleware would.
func evalSurface(t *testing.T, route string, request *http.Request) verdict {
	t.Helper()
	tree, ok := RouteTree(route)
	if !ok {
		t.Fatalf("route %q is bound to no surface", route)
	}
	compiled, err := Compile(Optimize(tree, DefaultPipeline()))
	if err != nil {
		t.Fatalf("compile %q: %v", route, err)
	}
	return compiled.eval(request.Context(), newRequestPolicyInput(request, compiled.bodyBudget))
}

// TestOwnershipProbesOncePerRequest pins the shape of the cost: the fact is
// loaded once for the request that needs it, not once per leaf that mentions it
// and not once per evaluation of the tree.
func TestOwnershipProbesOncePerRequest(t *testing.T) {
	facts := installCountingFacts(t, "ns-a")
	user := &User{ID: "u-1", AZP: "cyclops-cs-spa", PrincipalType: PrincipalTypeUser}

	result := evalSurface(t, svcPathRoute,
		characterizationRequest(svcPathRoute, svcCase("ns-a"), http.MethodGet, user))
	if result.truth != truthTrue {
		t.Fatalf("verdict = %v, want allow", verdictName(result.truth))
	}
	if got := facts.loads.Load(); got != 1 {
		t.Fatalf("fact loads = %d, want exactly 1 per request", got)
	}
}

// The per-request cache is what makes "once per request" hold when more than
// one leaf reads the same provider. The production ownership tree has a single
// fact leaf, so the property is pinned on a tree that has two — otherwise
// nothing would notice the cache disappearing until a second consumer arrived.
func TestOwnershipFactCacheIsPerRequestNotPerLeaf(t *testing.T) {
	facts := installCountingFacts(t, "ns-a")
	leaf := func() Node {
		return Policy(
			Modules(Registered("authz"), Registered("authz-ownership")),
			Query("data.authz_ownership.rbac_allow"),
			WithFacts(NamespaceRBACFactNamespace, RegisteredFacts(NamespaceRBACFactProvider)),
		)
	}
	compiled, err := Compile(All(leaf(), leaf(), leaf()))
	if err != nil {
		t.Fatalf("compile: %v", err)
	}
	request := characterizationRequest(svcPathRoute, svcCase("ns-a"), http.MethodGet,
		&User{ID: "u-1", AZP: "cyclops-cs-spa", PrincipalType: PrincipalTypeUser})
	if result := compiled.eval(request.Context(), newRequestPolicyInput(request, compiled.bodyBudget)); result.truth != truthTrue {
		t.Fatalf("verdict = %v, want allow", verdictName(result.truth))
	}
	if got := facts.loads.Load(); got != 1 {
		t.Fatalf("fact loads = %d across three leaves sharing a cache key, want 1", got)
	}
}

// TestOwnershipCheapDenyProbesNothing is the effectiveness the planner was
// built for, on a production route. Each of these principals is denied by a
// leaf that costs nothing — the base policy, the surface's DNS-label check, or
// the ownership conjunct's own probe_eligible — and every one of them must cost
// zero probes, because the lazy fold never reaches the leaf carrying the
// provider.
//
// The provider is set to allow the namespace being asked about, so a probe that
// did happen would be visible as a verdict change as well as a count.
func TestOwnershipCheapDenyProbesNothing(t *testing.T) {
	for _, testCase := range []struct {
		name      string
		user      *User
		routeCase routeCase
		deniedBy  string
	}{
		{
			name:      "empty subject",
			user:      &User{ID: "", AZP: "cyclops-cs-spa", PrincipalType: PrincipalTypeUser},
			routeCase: svcCase("ns-b"),
			deniedBy:  "authz_base",
		},
		{
			name:      "invalid namespace label",
			user:      &User{ID: "u-1", AZP: "cyclops-cs-spa", PrincipalType: PrincipalTypeUser},
			routeCase: svcCase("Not_A_Label"),
			deniedBy:  "authz_svc",
		},
		{
			name: "per-key token outside its namespace",
			user: &User{ID: "svc-1", AZP: "key-ns-z", Namespace: "ns-z", PrincipalType: PrincipalTypeUser},
			// ns-b is neither the claim nor a namespace the provider allows.
			routeCase: svcCase("ns-b"),
			deniedBy:  "authz_svc",
		},
		{
			name: "GitHub token outside its grant",
			user: &User{
				ID:                "owner-1",
				AZP:               "github-oidc",
				PrincipalType:     PrincipalTypeGitHubOIDC,
				AllowedNamespaces: []string{"ns-a"},
			},
			routeCase: svcCase("ns-b"),
			deniedBy:  "authz_ownership.probe_eligible",
		},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			facts := installCountingFacts(t, testCase.routeCase.params["namespace"])
			result := evalSurface(t, svcPathRoute,
				characterizationRequest(svcPathRoute, testCase.routeCase, http.MethodGet, testCase.user))
			if result.truth != truthFalse {
				t.Fatalf("verdict = %v, want deny (denied by %s)", verdictName(result.truth), testCase.deniedBy)
			}
			if got := facts.loads.Load(); got != 0 {
				t.Fatalf("fact loads = %d, want 0: %s decides this request before the loader is reached",
					got, testCase.deniedBy)
			}
		})
	}
}

// And the same for a claim that allows: an Any that stops at its first true
// never reaches the branch holding the provider.
func TestOwnershipClaimAllowProbesNothing(t *testing.T) {
	for _, testCase := range []struct {
		name string
		user *User
	}{
		{
			name: "per-key token in its namespace",
			user: &User{ID: "svc-1", AZP: "key-ns-a", Namespace: "ns-a", PrincipalType: PrincipalTypeUser},
		},
		{
			name: "GitHub token inside its grant",
			user: &User{
				ID:                "owner-1",
				AZP:               "github-oidc",
				PrincipalType:     PrincipalTypeGitHubOIDC,
				AllowedNamespaces: []string{"ns-a"},
			},
		},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			// Deliberately answers "no" for ns-a: if a probe happened at all,
			// the verdict would flip as well as the count.
			facts := installCountingFacts(t, "nothing")
			result := evalSurface(t, svcPathRoute,
				characterizationRequest(svcPathRoute, svcCase("ns-a"), http.MethodGet, testCase.user))
			if result.truth != truthTrue {
				t.Fatalf("verdict = %v, want allow from the claim alone", verdictName(result.truth))
			}
			if got := facts.loads.Load(); got != 0 {
				t.Fatalf("fact loads = %d, want 0: the claim decides before the loader is reached", got)
			}
		})
	}
}

// TestSinkFactLoadersRemovesAProbeFromTheProductionTree is the measurement with
// its baseline. Production declares the cheap children first, so the pass moves
// nothing there and its effect cannot be seen by comparing the plan to itself.
// Reversing every composite's children models the same tree written the other
// way round: unoptimized it costs a probe, and the pass takes it back to zero.
// Same verdict throughout — that is the lattice's promise, and what makes the
// reordering legal in the first place.
func TestSinkFactLoadersRemovesAProbeFromTheProductionTree(t *testing.T) {
	tree, ok := RouteTree(svcPathRoute)
	if !ok {
		t.Fatalf("route %q is bound to no surface", svcPathRoute)
	}
	user := &User{
		ID:                "owner-1",
		AZP:               "github-oidc",
		PrincipalType:     PrincipalTypeGitHubOIDC,
		AllowedNamespaces: []string{"ns-a"},
	}

	run := func(t *testing.T, plan Node) (truth, int64) {
		t.Helper()
		facts := installCountingFacts(t, "ns-b")
		compiled, err := Compile(plan)
		if err != nil {
			t.Fatalf("compile: %v", err)
		}
		request := characterizationRequest(svcPathRoute, svcCase("ns-b"), http.MethodGet, user)
		result := compiled.eval(request.Context(), newRequestPolicyInput(request, compiled.bodyBudget))
		return result.truth, facts.loads.Load()
	}

	reversed := reverseChildren(tree)

	baselineTruth, baselineLoads := run(t, reversed)
	if baselineLoads != 1 {
		t.Fatalf("unoptimized reversed plan loaded facts %d times, want 1 — the baseline this pass is measured against", baselineLoads)
	}

	sunkTruth, sunkLoads := run(t, Optimize(reversed, DefaultPipeline()))
	if sunkLoads != 0 {
		t.Fatalf("optimized plan loaded facts %d times, want 0", sunkLoads)
	}
	if baselineTruth != sunkTruth {
		t.Fatalf("optimizing changed the verdict: %v -> %v", verdictName(baselineTruth), verdictName(sunkTruth))
	}
}

// reverseChildren rewrites a tree with every composite's children in reverse
// order. It is a way to write "the same policy, declared the other way round"
// without a second copy of the policy.
func reverseChildren(n Node) Node {
	reverse := func(children []Node) []Node {
		out := make([]Node, 0, len(children))
		for index := len(children) - 1; index >= 0; index-- {
			out = append(out, reverseChildren(children[index]))
		}
		return out
	}
	switch node := n.(type) {
	case AllNode:
		return AllNode{Children: reverse(node.Children)}
	case AnyNode:
		return AnyNode{Children: reverse(node.Children)}
	case BecauseNode:
		return BecauseNode{Child: reverseChildren(node.Child), Reason: node.Reason}
	default:
		return n
	}
}

// TestUnregisteredFactProviderFailsClosedAsAnInternalError pins the other half
// of the failure-mode split. An unreachable apiserver is a FactUnavailableError
// and a 502; a provider nobody registered is a plain error and a 500, because
// nothing was unreachable — the process was assembled wrong, and a retry will
// not help.
func TestUnregisteredFactProviderFailsClosedAsAnInternalError(t *testing.T) {
	LoadOpa()
	factProviderRegistry.RLock()
	previous, existed := factProviderRegistry.providers[NamespaceRBACFactProvider]
	factProviderRegistry.RUnlock()
	factProviderRegistry.Lock()
	delete(factProviderRegistry.providers, NamespaceRBACFactProvider)
	factProviderRegistry.Unlock()
	t.Cleanup(func() {
		if existed {
			RegisterFactProvider(NamespaceRBACFactProvider, previous)
		}
	})

	tree, _ := RouteTree(svcPathRoute)
	middleware := PolicyMiddleware(Optimize(tree, DefaultPipeline()))
	handler := middleware(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		t.Error("an undecidable policy reached the handler")
		w.WriteHeader(http.StatusOK)
	}))

	response := httptest.NewRecorder()
	handler.ServeHTTP(response, characterizationRequest(svcPathRoute, svcCase("ns-a"), http.MethodGet,
		&User{ID: "u-1", AZP: "cyclops-cs-spa", PrincipalType: PrincipalTypeUser}))

	if response.Code != http.StatusInternalServerError {
		t.Fatalf("status = %d, want 500; body = %s", response.Code, response.Body.String())
	}
}

// TestOwnedNamespaceMatchesRego holds the two halves of target_namespace
// together. The Go half is what the provider probes and the Rego half is what
// the claims are compared against; if they ever disagreed, a token could be
// checked against one namespace and a probe run for another, which is a
// confused-deputy bug rather than a tidiness one.
func TestOwnedNamespaceMatchesRego(t *testing.T) {
	query := prepareQuery(t, "data.authz_ownership.target_namespace", map[string]string{
		"authz.rego":           authzPolicy,
		"authz_ownership.rego": authzOwnershipPolicy,
	})

	for _, testCase := range []struct {
		name   string
		params map[string]string
		want   string
	}{
		{name: "svc namespace", params: map[string]string{"namespace": "ns-a", "service": "svc-a"}, want: "ns-a"},
		{name: "namespaces name", params: map[string]string{"name": "ns-b"}, want: "ns-b"},
		{name: "empty namespace stays empty", params: map[string]string{"namespace": "", "service": "svc-a"}, want: ""},
		{name: "empty name stays empty", params: map[string]string{"name": ""}, want: ""},
		{name: "namespace wins over name", params: map[string]string{"namespace": "ns-a", "name": "ns-z"}, want: "ns-a"},
		{name: "no namespace parameter at all", params: map[string]string{"path": "x"}, want: ""},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			ctx := context.WithValue(context.Background(), paramsKey, testCase.params)
			if got := OwnedNamespace(ctx); got != testCase.want {
				t.Fatalf("OwnedNamespace = %q, want %q", got, testCase.want)
			}

			results, err := query.Eval(context.Background(), rego.EvalInput(map[string]any{"params": testCase.params}))
			if err != nil {
				t.Fatalf("eval target_namespace: %v", err)
			}
			// An undefined rule and "" are the same answer for every rule that
			// reads it: all of them require a non-empty namespace.
			got := ""
			if len(results) > 0 && len(results[0].Expressions) > 0 {
				got, _ = results[0].Expressions[0].Value.(string)
			}
			if got != testCase.want {
				t.Fatalf("target_namespace = %q, want %q (OwnedNamespace said %q)",
					got, testCase.want, OwnedNamespace(ctx))
			}
		})
	}
}

// TestLeavesReadingFactsDeclareAProvider is the invariant
// TestRouteModulesNeverReadInputFacts stated while no module read facts, in the
// form that survives one of them starting to. It is asked per leaf rather than
// per module, and transitively rather than syntactically, because
// authz_ownership.rego is compiled into three leaves and only one of them
// queries the rule that reaches input.facts — a module-level answer would
// condemn the other two, and a syntactic one would miss a fact read behind a
// helper rule.
//
// A leaf whose query depends on input.facts and declares no provider is handed
// {}: an undefined lookup that denies, silently, on every request.
func TestLeavesReadingFactsDeclareAProvider(t *testing.T) {
	LoadOpa()
	factsRef := ast.MustParseRef("input.facts")

	for _, route := range AuthenticatedRoutes() {
		tree, ok := RouteTree(route)
		if !ok {
			t.Fatalf("route %q is bound to no surface", route)
		}
		for _, leaf := range allLeaves(tree) {
			modules, err := leaf.Source.modules()
			if err != nil {
				t.Fatalf("load modules for %q: %v", leaf.Query, err)
			}
			parsed := map[string]*ast.Module{}
			for _, module := range modules {
				module, err := ast.ParseModule(module.name, module.source)
				if err != nil {
					t.Fatalf("parse %s: %v", module.Package.Location.File, err)
				}
				parsed[module.Package.Path.String()] = module
			}
			compiler := ast.NewCompiler()
			if compiler.Compile(parsed); compiler.Failed() {
				t.Fatalf("compile modules for %q: %v", leaf.Query, compiler.Errors)
			}

			query, err := ast.ParseBody(leaf.Query)
			if err != nil {
				t.Fatalf("parse query %q: %v", leaf.Query, err)
			}
			refs, err := dependencies.Base(compiler, query)
			if err != nil {
				t.Fatalf("dependencies of %q: %v", leaf.Query, err)
			}
			readsFacts := false
			for _, ref := range refs {
				if ref.HasPrefix(factsRef) {
					readsFacts = true
				}
			}
			if readsFacts && len(leaf.Facts) == 0 {
				t.Errorf(`route %q runs a leaf querying %q, which depends on input.facts but
declares no fact provider. That leaf is evaluated against an empty facts
document, so the lookup is undefined and the leaf denies every request.`,
					route, leaf.Query)
			}
			if !readsFacts && len(leaf.Facts) > 0 {
				t.Errorf(`route %q runs a leaf querying %q, which declares a fact provider
its query never reads. The load is paid for on every request that reaches the
leaf and its result is discarded.`, route, leaf.Query)
			}
		}
	}
}

// allLeaves flattens a tree into its leaves whatever the combinators are —
// unlike conjunctiveLeaves in policy_test.go, which refuses a disjunction
// because it is folding one.
func allLeaves(n Node) []Leaf {
	switch node := n.(type) {
	case Leaf:
		return []Leaf{node}
	case AllNode:
		var leaves []Leaf
		for _, child := range node.Children {
			leaves = append(leaves, allLeaves(child)...)
		}
		return leaves
	case AnyNode:
		var leaves []Leaf
		for _, child := range node.Children {
			leaves = append(leaves, allLeaves(child)...)
		}
		return leaves
	case BecauseNode:
		return allLeaves(node.Child)
	}
	return nil
}
