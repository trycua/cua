package auth

import (
	"fmt"
	"slices"
	"sync"
)

// The policy trees production routes are built from.
//
// They live here rather than in main.go so that exactly one definition exists:
// main.go's route constructors call these, and policy_golden_test.go's snapshot
// table names these same functions. A tree defined in main.go and copied into
// the test would let production authorization be reordered with the golden
// still green — the copy, not the route, is what such a snapshot pins.
//
// Registered() resolves through the module registry that LoadOpa populates, so
// these constructors stay pure data: no I/O, no compilation, safe to call
// before LoadOpa. A name that was never registered surfaces at Compile time,
// which for a route means PolicyMiddleware panicking during construction.

// Every route policy is All(BasePolicy(), <its surface>). That is not a
// convention imposed on the Rego — it is the Rego's own structure, made visible.
// All 30 route-gated allow rules of the former monolith shared exactly one
// condition, a non-empty subject, and each was gated to a route belonging to one
// surface. So for a request on route R,
//
//	All(base, surface_R)  ≡  the old authz.allow
//
// with base the shared condition and surface_R the disjunction of R's rules
// minus it. Rules of another surface could not have fired on R in the first
// place, because their route gates would not match; the surfaces' route sets are
// pairwise disjoint, the only prefix rule (billing) overlapping no other
// surface's literals. auth/testdata/route-authorization-table.txt is the
// executable form of that argument: it was recorded against the monolith, and
// the split had to leave every cell of it unmoved.

// BasePolicy is the check every authenticated route runs whatever its surface:
// a non-empty subject. Denies are a 403 and an undecidable evaluation is a 500,
// both from PolicyMiddleware.
func BasePolicy() Node {
	return Policy(
		Registered("authz-base"),
		Query("data.authz_base.allow"),
	)
}

// surfaceLeaf builds the leaf for one surface module. Every surface names
// authz.rego alongside its own module, because every surface module imports the
// principal vocabulary defined there.
func surfaceLeaf(module, query string) Node {
	return Policy(
		Modules(
			Registered("authz"),
			Registered(module),
		),
		Query(query),
	)
}

// KeysRoutePolicy guards /api/keys and /api/keys/{id}.
func KeysRoutePolicy() Node {
	return All(BasePolicy(), surfaceLeaf("authz-keys", "data.authz_keys.allow"))
}

// ConfigRoutePolicy guards /api/config.
func ConfigRoutePolicy() Node {
	return All(BasePolicy(), surfaceLeaf("authz-config", "data.authz_config.allow"))
}

// ChatRoutePolicy guards browser-local Bash conversation routes.
func ChatRoutePolicy() Node {
	return All(BasePolicy(), surfaceLeaf("authz-chat", "data.authz_chat.allow"))
}

// BillingRoutePolicy guards the Stripe-hosted billing browser routes. Its module
// matches a prefix rather than three literals, so a billing route added to
// main.go and bound here is covered without a policy change.
func BillingRoutePolicy() Node {
	return All(BasePolicy(), surfaceLeaf("authz-billing", "data.authz_billing.allow"))
}

func UsageRoutePolicy() Node {
	return All(BasePolicy(), surfaceLeaf("authz-usage", "data.authz_usage.allow"))
}

// NamespaceOwnershipPolicy is the tenancy boundary for routes that name a
// namespace in their path. It is a conjunct rather than a surface, because
// three surfaces carry it and one of them (namespaces) holds routes on both
// sides of it — authz_ownership.rego decides which.
//
// The shape is the point:
//
//	Any(claim_allow, All(probe_eligible, rbac_allow))
//
// Only rbac_allow declares a fact provider, and it is the only leaf that costs
// a network round trip. Both combinators are lazy and both are guarded by a
// cheap sibling, so the probe runs exactly when neither the token's claims nor
// the principal's family has already answered:
//
//   - Any stops at the first true, so a GitHub or per-key principal whose claim
//     matches never reaches the second child at all.
//   - All stops at the first false, so a principal a K8s probe cannot speak for
//     — a GitHub token, a per-key service account — denies before the loader.
//
// That laziness is what sinkFactLoaders exists to guarantee: the declaration
// order below already puts the cheap child first, and the pass is what keeps it
// there under any future rewrite. policy_ownership_test.go measures both claims
// as probe counts rather than restating them.
func NamespaceOwnershipPolicy() Node {
	leaf := func(query string, options ...PolicyOption) Node {
		return Policy(
			Modules(
				Registered("authz"),
				Registered("authz-ownership"),
			),
			append([]PolicyOption{Query(query)}, options...)...,
		)
	}
	return Any(
		leaf("data.authz_ownership.claim_allow"),
		All(
			leaf("data.authz_ownership.probe_eligible"),
			leaf(
				"data.authz_ownership.rbac_allow",
				WithFacts(NamespaceRBACFactNamespace, RegisteredFacts(NamespaceRBACFactProvider)),
			),
		),
	)
}

// NamespacesRoutePolicy guards /api/namespaces and /api/namespaces/{name}. The
// ownership conjunct only binds on GET /api/namespaces/{name}; the list, create
// and delete routes pass it unconditionally, which is where the Go handler left
// them.
func NamespacesRoutePolicy() Node {
	return All(
		BasePolicy(),
		surfaceLeaf("authz-namespaces", "data.authz_namespaces.allow"),
		NamespaceOwnershipPolicy(),
	)
}

// GitHubTrustRoutePolicy guards the GitHub trust-policy routes.
func GitHubTrustRoutePolicy() Node {
	return All(BasePolicy(), surfaceLeaf("authz-github-trust", "data.authz_github_trust.allow"))
}

// UserKeysRoutePolicy guards /api/user-keys and /api/user-keys/{id}.
func UserKeysRoutePolicy() Node {
	return All(BasePolicy(), surfaceLeaf("authz-user-keys", "data.authz_user_keys.allow"))
}

// SvcRoutePolicy guards the generic service proxy, both with and without a
// trailing path. The ownership conjunct is its direct-dial tenancy boundary.
func SvcRoutePolicy() Node {
	return All(
		BasePolicy(),
		surfaceLeaf("authz-svc", "data.authz_svc.allow"),
		NamespaceOwnershipPolicy(),
	)
}

func SignedServiceURLsRoutePolicy() Node {
	return All(
		BasePolicy(),
		surfaceLeaf("authz-signed-service-urls", "data.authz_signed_service_urls.allow"),
		NamespaceOwnershipPolicy(),
	)
}

// StateQueryRoutePolicy guards /api/state/query.
func StateQueryRoutePolicy() Node {
	return All(BasePolicy(), surfaceLeaf("authz-state-query", "data.authz_state_query.allow"))
}

const billingSetupRequiredMessage = "A payment method is required to create this resource. Add one in Billing and try again."

// FeatureFlagsRoutePolicy guards the admin-only feature-flag management API.
func FeatureFlagsRoutePolicy() Node {
	return All(BasePolicy(), surfaceLeaf("authz-feature-flags", "data.authz_feature_flags.allow"))
}

// K8sRoutePolicy guards /api/k8s/{path...}. It is the same base + surface shape
// as every other route, with two admission conjuncts: card-or-admin admission
// for custom-resource creation, and pool admission over the request body.
//
// Every conjunct must pass. The pool-admission leaf reads the raw body (bounded at 1 MiB)
// to inspect the object being created or patched, which is why it names
// pool_admission.rego alongside authz.rego — pool_admission imports
// data.authz.is_admin. It stays a separate leaf rather than rules inside
// authz_k8s.rego because it is the only thing on this surface that needs the
// body, and folding it in would put every k8s request's verdict behind a body
// read.
func CustomResourceCreationAdmissionPolicy() Node {
	leaf := func(query string, options ...PolicyOption) Node {
		return Policy(
			Modules(
				Registered("authz"),
				Registered("custom-resource-creation-admission"),
			),
			append([]PolicyOption{Query(query)}, options...)...,
		)
	}
	return Any(
		leaf("data.custom_resource_creation_admission.exempt"),
		leaf(
			"data.custom_resource_creation_admission.billing_eligible",
			WithFacts(StripeCardsFactNamespace, RegisteredFacts(StripeCardsFactProvider)),
			WithFacts(TimeFactNamespace, RegisteredFacts(CurrentYearFactProvider)),
			WithFacts(TimeFactNamespace, RegisteredFacts(CurrentMonthFactProvider)),
		),
	)
}

func K8sRoutePolicy() Node {
	return All(
		BasePolicy(),
		surfaceLeaf("authz-k8s", "data.authz_k8s.allow"),
		Because(CustomResourceCreationAdmissionPolicy(), billingSetupRequiredMessage),
		Policy(
			Modules(
				Registered("authz"),
				Registered("pool-admission"),
			),
			Query("data.pool_admission.allow"),
			WithRawBody(1<<20),
		),
	)
}

// ── Route → surface binding ─────────────────────────────────────────────────
//
// A route's authorization is chosen here, by name, rather than at its
// registration site. Before this table existed, main.go decided which policy a
// route ran and authz.rego decided which routes it reasoned about, and nothing
// held the two together — #6702 deleted the /api/gateway route and its
// authorization rules outlived it, because there was no place where the two
// facts met. The table is that place: it is what main.go reads, what
// policy_route_correspondence_test.go checks against the registered routes and
// against the Rego, and what the characterization table in
// policy_characterization_test.go enumerates.
//
// A route missing from this table cannot serve traffic — RouteMiddleware panics
// during route construction rather than falling back to a default, because the
// only safe default for "no policy named" is refusing to start.

// surfacePolicy is one authorization surface: the tree its routes run and the
// middleware options that tree is installed with. Options live here rather than
// at the call site so that two routes on one surface cannot disagree about, say,
// the denial message.
type surfacePolicy struct {
	tree    func() Node
	options []MiddlewareOption
}

const featureFlagAuditBodyLimit = 64 << 10

// surfacePolicies is every surface, by name. A surface owning no route fails
// TestEveryPolicySurfaceOwnsARoute; a route naming no surface cannot start.
var surfacePolicies = map[string]surfacePolicy{
	"keys":                {tree: KeysRoutePolicy},
	"config":              {tree: ConfigRoutePolicy},
	"chat":                {tree: ChatRoutePolicy},
	"billing":             {tree: BillingRoutePolicy},
	"usage":               {tree: UsageRoutePolicy},
	"namespaces":          {tree: NamespacesRoutePolicy},
	"github-trust":        {tree: GitHubTrustRoutePolicy},
	"user-keys":           {tree: UserKeysRoutePolicy},
	"svc":                 {tree: SvcRoutePolicy},
	"signed-service-urls": {tree: SignedServiceURLsRoutePolicy},
	"state-query":         {tree: StateQueryRoutePolicy},
	"feature-flags":       {tree: FeatureFlagsRoutePolicy, options: []MiddlewareOption{WithDeniedAudit("feature_flag_admin", featureFlagAuditBodyLimit), WithAdminAPIErrorResponses(), WithFreshAdminAuthorization()}},
	"k8s": {
		tree:    K8sRoutePolicy,
		options: []MiddlewareOption{WithDeniedMessage("k8s request is not allowed")},
	},
}

// routeSurfaces maps every authenticated route pattern to its surface. The keys
// are the route names main.go stamps via RouteContext, which is also what the
// policy matches on as input.route.
//
// Several routes to one surface is the normal case, and is why plans are
// memoized per surface: the three billing routes share one module, one tree,
// and one compiled plan.
var routeSurfaces = map[string]string{
	"/api/config":                "config",
	"/api/analytics/session":     "config",
	"/api/analytics/attribution": "config",
	"/api/state/query":           "state-query",
	"/api/usage/overview":        "usage",
	"/api/usage/pool":            "usage",
	"/api/usage/browser-timings": "usage",

	"/api/chat/conversations":            "chat",
	"/api/chat/conversations/{id}":       "chat",
	"/api/chat/conversations/{id}/turns": "chat",

	"/api/billing/summary":        "billing",
	"/api/billing/usage":          "billing",
	"/api/billing/setup-session":  "billing",
	"/api/billing/portal-session": "billing",

	"/api/keys":      "keys",
	"/api/keys/{id}": "keys",

	"/api/namespaces":        "namespaces",
	"/api/namespaces/{name}": "namespaces",

	"/api/user-keys":      "user-keys",
	"/api/user-keys/{id}": "user-keys",

	"/api/github-trust-policies":      "github-trust",
	"/api/github-trust-policies/{id}": "github-trust",

	"/api/svc/{namespace}/{service}":           "svc",
	"/api/svc/{namespace}/{service}/{path...}": "svc",

	"/api/signed-service-urls/{namespace}":      "signed-service-urls",
	"/api/signed-service-urls/{namespace}/{id}": "signed-service-urls",

	"/api/k8s/{path...}":             "k8s",
	"/api/admin/feature-flags":       "feature-flags",
	"/api/admin/feature-flags/{key}": "feature-flags",
}

// AuthenticatedRoutes returns every route the policy layer covers, sorted.
func AuthenticatedRoutes() []string {
	routes := make([]string, 0, len(routeSurfaces))
	for route := range routeSurfaces {
		routes = append(routes, route)
	}
	slices.Sort(routes)
	return routes
}

// SurfaceNames returns every surface name, sorted.
func SurfaceNames() []string {
	names := make([]string, 0, len(surfacePolicies))
	for name := range surfacePolicies {
		names = append(names, name)
	}
	slices.Sort(names)
	return names
}

// RouteSurface returns the surface a route's authorization belongs to.
func RouteSurface(route string) (string, bool) {
	name, ok := routeSurfaces[route]
	return name, ok
}

// SurfaceTree returns the policy tree a surface runs. The tree is rebuilt per
// call, like every other constructor here, so a caller that rewrites one cannot
// leak the rewrite into the next.
func SurfaceTree(name string) (Node, bool) {
	surface, ok := surfacePolicies[name]
	if !ok {
		return nil, false
	}
	return surface.tree(), true
}

// RouteTree returns the policy tree a route runs.
func RouteTree(route string) (Node, bool) {
	name, ok := RouteSurface(route)
	if !ok {
		return nil, false
	}
	return SurfaceTree(name)
}

// surfaceMiddlewares memoizes one compiled plan per surface. PolicyMiddleware
// optimizes and compiles eagerly, so building it per route would prepare the
// same Rego once per route sharing a surface — 17 plans rather than 10.
//
// What matters for a long-lived sidecar is the heap, because each plan holds a
// prepared query for the whole of the process's life. Measured on this tree:
// 10 plans retain 3.18 MiB, one retains 0.24 MiB, and memoizing per route
// instead would retain 4.95 MiB for the same 17 routes. The split into surfaces
// cost 0.75 MiB against the single-module policy's two plans (2.43 MiB) and
// nothing in compile time — 11.6ms against 12.6ms, because the same Rego is
// prepared either way, just cut into ten pieces. Memoization also saves main_test.go
// the redundant compiles of building a router four times.
//
// Memoization is lazy rather than a package-level var because Registered("authz")
// resolves through the module registry at compile time and LoadOpa is what
// populates that registry. Deferring to first use puts the compile after LoadOpa,
// and a name still unregistered by then panics during route construction rather
// than failing a request.
var (
	surfaceMiddlewareMu sync.Mutex
	surfaceMiddlewares  = map[string]Middleware{}
)

// SurfaceMiddleware returns the authorization middleware for a surface,
// compiling its plan at most once per process. An unknown name panics: it can
// only come from a route bound to a surface nobody defined, which is a
// construction-time mistake with nothing downstream to notice it.
func SurfaceMiddleware(name string) Middleware {
	surfaceMiddlewareMu.Lock()
	defer surfaceMiddlewareMu.Unlock()
	if middleware, ok := surfaceMiddlewares[name]; ok {
		return middleware
	}
	surface, ok := surfacePolicies[name]
	if !ok {
		panic(fmt.Sprintf("no policy surface named %q", name))
	}
	middleware := PolicyMiddleware(surface.tree(), surface.options...)
	surfaceMiddlewares[name] = middleware
	return middleware
}

// RouteMiddleware returns the authorization middleware a route runs. A route
// with no surface panics during route construction — an authenticated route
// whose authorization nobody chose must not start, and there is no safe default
// to fall back to.
func RouteMiddleware(route string) Middleware {
	name, ok := RouteSurface(route)
	if !ok {
		panic(fmt.Sprintf("route %q has no authorization surface; add it to routeSurfaces in auth/policy_routes.go", route))
	}
	return SurfaceMiddleware(name)
}
