package main

import (
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"

	"cyclops-cs-backend/auth"

	regoast "github.com/open-policy-agent/opa/ast"
)

// Three descriptions of the same route set have to agree, and until this file
// existed nothing made them:
//
//   1. what setupRouter registers,
//   2. what auth/policy_routes.go binds to an authorization surface,
//   3. what the Rego reasons about as input.route.
//
// #6702 deleted the /api/gateway route from (1) and its rules survived in (3)
// until a human noticed. This test is the thing that notices. It reads all three
// from source rather than from a copy: setupRouter's AST, the exported route
// table, and the parsed Rego modules.
//
// The checks run in both directions on purpose. A route registered with no
// policy is an open endpoint; a policy naming a route nobody registers is dead
// authorization code that reads as protection and provides none.

// routeWrappers is every helper setupRouter may wrap a handler in, and whether
// that helper installs the authorization stage. A wrapper missing from this map
// fails the test rather than being skipped: a new chain that quietly bypasses
// the policy is precisely the drift this file exists to catch.
var routeWrappers = map[string]bool{
	"withAuthenticatedMiddlewares": true,
	"onlyLog":                      false,
}

type registeredRoute struct {
	route         string
	wrapper       string
	authenticated bool
}

// registeredRoutes reads setupRouter's body and returns the route name each
// r.Handle call passes to its wrapper. That name — not the mux pattern — is what
// RouteContext stamps and what the policy matches as input.route, so it is the
// only one of the two worth comparing against a policy.
func registeredRoutes(t *testing.T) []registeredRoute {
	t.Helper()

	fileSet := token.NewFileSet()
	file, err := parser.ParseFile(fileSet, "main.go", nil, parser.ParseComments)
	if err != nil {
		t.Fatalf("parse main.go: %v", err)
	}

	var setupRouterDecl *ast.FuncDecl
	for _, decl := range file.Decls {
		if function, ok := decl.(*ast.FuncDecl); ok && function.Name.Name == "setupRouter" && function.Recv == nil {
			setupRouterDecl = function
			break
		}
	}
	if setupRouterDecl == nil {
		t.Fatal("main.go has no setupRouter function; this test reads the route table out of its body")
	}

	var routes []registeredRoute
	ast.Inspect(setupRouterDecl.Body, func(n ast.Node) bool {
		call, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		selector, ok := call.Fun.(*ast.SelectorExpr)
		if !ok || selector.Sel.Name != "Handle" {
			return true
		}
		if receiver, ok := selector.X.(*ast.Ident); !ok || receiver.Name != "r" {
			return true
		}
		if len(call.Args) != 2 {
			t.Errorf("r.Handle at %s takes %d arguments; expected a pattern and a handler",
				fileSet.Position(call.Pos()), len(call.Args))
			return true
		}
		wrapper, ok := call.Args[1].(*ast.CallExpr)
		if !ok {
			t.Errorf("r.Handle at %s does not wrap its handler in a known chain; every route must go through one of %v",
				fileSet.Position(call.Pos()), sortedWrapperNames())
			return true
		}
		name, ok := wrapper.Fun.(*ast.Ident)
		if !ok {
			t.Errorf("r.Handle at %s wraps its handler in a non-identifier call; every route must go through one of %v",
				fileSet.Position(call.Pos()), sortedWrapperNames())
			return true
		}
		authenticated, known := routeWrappers[name.Name]
		if !known {
			t.Errorf(`r.Handle at %s wraps its handler in %s, which routeWrappers does not know about.
Add it to routeWrappers with whether it installs the authorization stage. A new
chain that skips the policy must not be able to reach production by being
unrecognised here.`, fileSet.Position(call.Pos()), name.Name)
			return true
		}
		if len(wrapper.Args) == 0 {
			t.Errorf("%s at %s takes no route name", name.Name, fileSet.Position(wrapper.Pos()))
			return true
		}
		literal, ok := wrapper.Args[0].(*ast.BasicLit)
		if !ok || literal.Kind != token.STRING {
			t.Errorf("%s at %s is given a non-literal route name; the route table can only be read from literals",
				name.Name, fileSet.Position(wrapper.Pos()))
			return true
		}
		route, err := strconv.Unquote(literal.Value)
		if err != nil {
			t.Errorf("unquote route name %s at %s: %v", literal.Value, fileSet.Position(literal.Pos()), err)
			return true
		}
		routes = append(routes, registeredRoute{route: route, wrapper: name.Name, authenticated: authenticated})
		return true
	})

	if len(routes) == 0 {
		t.Fatal("read no routes out of setupRouter; the AST shape this test relies on has changed")
	}
	return routes
}

func sortedWrapperNames() []string {
	names := make([]string, 0, len(routeWrappers))
	for name := range routeWrappers {
		names = append(names, name)
	}
	slices.Sort(names)
	return names
}

// policyRouteSet is what the Rego reasons about: the route names it compares
// against, the prefixes it matches, and the routes a prefix rule excludes.
//
// Exclusions are collected module-wide rather than per rule, so the model reads
// `startswith(input.route, "/api/billing/")` and `input.route !=
// "/api/billing/webhook"` as "the billing prefix minus the webhook" even though
// nothing here checks the two share a rule body. That is exact for the one
// prefix rule that exists, and it errs towards reporting a route as uncovered —
// the safe direction, since a false "uncovered" is a failing test and a false
// "covered" is an unguarded endpoint.
type policyRouteSet struct {
	literals map[string][]string
	prefixes map[string][]string
	excluded map[string][]string
}

func (set policyRouteSet) covers(route string) bool {
	if _, ok := set.excluded[route]; ok {
		return false
	}
	if _, ok := set.literals[route]; ok {
		return true
	}
	for prefix := range set.prefixes {
		if strings.HasPrefix(route, prefix) {
			return true
		}
	}
	return false
}

var inputRouteRef = regoast.MustParseRef("input.route")

// policyRoutes parses every non-test Rego module in auth/ and extracts the ways
// each one constrains input.route. Reading the directory rather than a list of
// filenames means a surface module added without being named here is still
// covered.
func policyRoutes(t *testing.T) policyRouteSet {
	t.Helper()

	paths, err := filepath.Glob(filepath.Join("auth", "*.rego"))
	if err != nil {
		t.Fatalf("glob auth/*.rego: %v", err)
	}
	set := policyRouteSet{
		literals: map[string][]string{},
		prefixes: map[string][]string{},
		excluded: map[string][]string{},
	}
	modules := 0
	for _, path := range paths {
		if strings.HasSuffix(path, "_test.rego") {
			continue
		}
		modules++
		module, err := regoast.ParseModule(path, readFileForTest(t, path))
		if err != nil {
			t.Fatalf("parse %s: %v", path, err)
		}
		collectRouteConstraints(t, path, module, set)
	}
	if modules == 0 {
		t.Fatal("found no Rego modules under auth/")
	}
	return set
}

func collectRouteConstraints(t *testing.T, path string, module *regoast.Module, set policyRouteSet) {
	t.Helper()

	regoast.WalkExprs(module, func(expr *regoast.Expr) bool {
		if !mentionsRoute(expr) {
			return false
		}
		operator, operands := callParts(expr)
		literal, matched := "", false
		if len(operands) == 2 {
			switch operator {
			case "equal", "eq", "neq":
				if operands[0].Value.Compare(inputRouteRef) == 0 {
					literal, matched = stringValue(operands[1])
				}
			case "startswith":
				if operands[0].Value.Compare(inputRouteRef) == 0 {
					literal, matched = stringValue(operands[1])
				}
			}
		}
		if !matched {
			t.Errorf(`%s constrains input.route in a form this test cannot read: %v
Recognised forms are input.route == "x", input.route != "x", and
startswith(input.route, "p"). Teach this test the new form, or the route
correspondence it checks silently stops covering that rule.`, path, expr)
			return false
		}
		if expr.Negated {
			t.Errorf(`%s negates a route constraint (%v). The coverage model here is a
plain union of literals and prefixes and cannot represent that.`, path, expr)
			return false
		}
		switch operator {
		case "equal", "eq":
			set.literals[literal] = appendOnce(set.literals[literal], path)
		case "neq":
			set.excluded[literal] = appendOnce(set.excluded[literal], path)
		case "startswith":
			set.prefixes[literal] = appendOnce(set.prefixes[literal], path)
		}
		return false
	})
}

func mentionsRoute(expr *regoast.Expr) bool {
	found := false
	regoast.WalkRefs(expr, func(ref regoast.Ref) bool {
		if ref.Compare(inputRouteRef) == 0 {
			found = true
		}
		return found
	})
	return found
}

func callParts(expr *regoast.Expr) (string, []*regoast.Term) {
	terms, ok := expr.Terms.([]*regoast.Term)
	if !ok || len(terms) == 0 {
		return "", nil
	}
	ref, ok := terms[0].Value.(regoast.Ref)
	if !ok {
		return "", nil
	}
	return strings.TrimPrefix(ref.String(), "internal."), terms[1:]
}

func stringValue(term *regoast.Term) (string, bool) {
	value, ok := term.Value.(regoast.String)
	if !ok {
		return "", false
	}
	return string(value), true
}

func appendOnce(existing []string, value string) []string {
	if slices.Contains(existing, value) {
		return existing
	}
	return append(existing, value)
}

func readFileForTest(t *testing.T, path string) string {
	t.Helper()
	contents, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read %s: %v", path, err)
	}
	return string(contents)
}

// TestRegisteredRoutesHaveAnAuthorizationSurface checks setupRouter against the
// route → surface table every authenticated route is dispatched through. A route
// registered but unbound cannot start (RouteMiddleware panics), so this direction
// mostly reports the failure earlier and by name; a surface bound to a route
// nobody registers is the direction with no runtime symptom at all.
func TestRegisteredRoutesHaveAnAuthorizationSurface(t *testing.T) {
	registered := registeredRoutes(t)

	fromRouter := map[string]bool{}
	for _, route := range registered {
		if route.authenticated {
			fromRouter[route.route] = true
		}
	}

	fromTable := map[string]bool{}
	for _, route := range auth.AuthenticatedRoutes() {
		fromTable[route] = true
	}

	for route := range fromRouter {
		if !fromTable[route] {
			t.Errorf("setupRouter registers %q behind the authorization stage, but routeSurfaces in auth/policy_routes.go does not bind it to a surface", route)
		}
	}
	for route := range fromTable {
		if !fromRouter[route] {
			t.Errorf("routeSurfaces binds %q to a surface, but setupRouter registers no such route; delete the binding or restore the route", route)
		}
	}

	for _, route := range registered {
		if route.authenticated {
			continue
		}
		if fromTable[route.route] {
			t.Errorf("%q is registered through %s, which installs no authorization stage, yet routeSurfaces binds it to a surface", route.route, route.wrapper)
		}
	}
}

// TestPolicyAndRouterAgreeOnRoutes is the check that would have caught #6702.
// The gateway route left main.go and its allow rules stayed; nothing compared
// the two, so the rules read as protection for a surface that no longer existed.
func TestPolicyAndRouterAgreeOnRoutes(t *testing.T) {
	registered := registeredRoutes(t)
	policy := policyRoutes(t)

	authenticated := map[string]bool{}
	all := map[string]bool{}
	for _, route := range registered {
		all[route.route] = true
		if route.authenticated {
			authenticated[route.route] = true
		}
	}

	t.Run("every authenticated route is reasoned about", func(t *testing.T) {
		for route := range authenticated {
			if !policy.covers(route) {
				t.Errorf(`%q runs the authorization stage, but no Rego rule constrains
input.route to it. Its policy can only allow it by accident or deny it always.`, route)
			}
		}
	})

	t.Run("every route the policy names is registered", func(t *testing.T) {
		for literal, sources := range policy.literals {
			if !authenticated[literal] {
				t.Errorf(`%v compares input.route against %q, which setupRouter does not
register behind the authorization stage. This is dead authorization code: it
reads as protection for a surface that does not exist.`, sources, literal)
			}
		}
	})

	t.Run("every prefix the policy matches covers a registered route", func(t *testing.T) {
		for prefix, sources := range policy.prefixes {
			covered := 0
			for route := range authenticated {
				if strings.HasPrefix(route, prefix) {
					covered++
				}
			}
			if covered == 0 {
				t.Errorf("%v matches input.route against the prefix %q, which no registered authenticated route starts with", sources, prefix)
			}
		}
	})

	t.Run("every route the policy excludes is registered", func(t *testing.T) {
		for literal, sources := range policy.excluded {
			if !all[literal] {
				t.Errorf(`%v excludes %q from a prefix rule, but setupRouter registers no
such route. A stale exclusion is a hole waiting for a route to be added back
under the same name.`, sources, literal)
			}
		}
	})

	t.Run("no unauthenticated route is covered", func(t *testing.T) {
		for _, route := range registered {
			if route.authenticated {
				continue
			}
			if policy.covers(route.route) {
				t.Errorf(`%q is registered through %s, which installs no authorization
stage, yet the policy reasons about it. Either the route needs the stage or the
rule is describing a boundary it does not enforce.`, route.route, route.wrapper)
			}
		}
	})
}

// TestEveryPolicySurfaceOwnsARoute catches a surface that survives the routes it
// was written for. A surface nobody dispatches to is a compiled plan no request
// can reach, and it looks exactly like one that is load-bearing.
func TestEveryPolicySurfaceOwnsARoute(t *testing.T) {
	owned := map[string]int{}
	for _, name := range auth.SurfaceNames() {
		owned[name] = 0
	}
	for _, route := range auth.AuthenticatedRoutes() {
		name, ok := auth.RouteSurface(route)
		if !ok {
			t.Fatalf("AuthenticatedRoutes returned %q, which RouteSurface does not resolve", route)
		}
		if _, known := owned[name]; !known {
			t.Errorf("route %q names surface %q, which SurfaceNames does not list", route, name)
			continue
		}
		owned[name]++
	}
	for name, count := range owned {
		if count == 0 {
			t.Errorf("surface %q owns no route; delete it or bind the routes it was written for", name)
		}
	}
}
