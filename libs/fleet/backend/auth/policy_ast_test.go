package auth

import (
	"fmt"
	"strings"
	"testing"
	"testing/fstest"
)

func TestASTIsPureDataAndDoesNotCompile(t *testing.T) {
	// Invalid Rego must NOT panic at construction. Compilation is deferred.
	node := Policy(
		Inline("broken.rego", "this is not valid rego at all"),
		Query("data.broken.allow"),
	)

	leaf, ok := node.(Leaf)
	if !ok {
		t.Fatalf("Policy returned %T, want Leaf", node)
	}
	if leaf.Query != "data.broken.allow" {
		t.Fatalf("Query = %q, want data.broken.allow", leaf.Query)
	}

	if _, err := Compile(node); err == nil {
		t.Fatal("Compile succeeded on invalid Rego, want error")
	}
}

func TestCompileReportsLeafFailuresAsErrors(t *testing.T) {
	// Rewrite passes build Leaf values directly, so Compile must stay
	// fallible for leaves that Policy's constructor guards would have caught.
	cases := []struct {
		name string
		leaf Leaf
		want string
	}{
		{
			name: "nil source",
			leaf: Leaf{Query: "data.p.allow"},
			want: `policy "data.p.allow" has no source`,
		},
		{
			name: "source fails to load",
			leaf: Leaf{Source: Registered("never-registered"), Query: "data.p.allow"},
			want: `load modules for policy "data.p.allow"`,
		},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			_, err := Compile(testCase.leaf)
			if err == nil {
				t.Fatal("Compile succeeded, want error")
			}
			if !strings.Contains(err.Error(), testCase.want) {
				t.Fatalf("error = %q, want it to contain %q", err, testCase.want)
			}
		})
	}
}

func TestCompileRejectsEmptyCompositeNodes(t *testing.T) {
	// All() and Any() panic on zero children, so this is unreachable today.
	// It stops being unreachable in the optimizer work: Children is exported
	// and rewrite passes build AllNode/AnyNode directly. A fold over zero
	// children returns the identity, so an empty AllNode compiled to an
	// unconditional allow — verified before this guard existed, an empty
	// AllNode served 200 from the downstream handler on every request.
	cases := []struct {
		name string
		node Node
		want string
	}{
		{name: "empty all", node: AllNode{}, want: "policy node auth.AllNode has no children"},
		{name: "empty any", node: AnyNode{}, want: "policy node auth.AnyNode has no children"},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			_, err := Compile(testCase.node)
			if err == nil {
				t.Fatal("Compile succeeded on an empty node, want error")
			}
			if !strings.Contains(err.Error(), testCase.want) {
				t.Fatalf("error = %q, want it to contain %q", err, testCase.want)
			}
		})
	}
}

func TestCompileWrapsSourceErrorForUnwrapping(t *testing.T) {
	_, err := Compile(Leaf{Source: Registered("never-registered"), Query: "data.p.allow"})
	if err == nil {
		t.Fatal("Compile succeeded, want error")
	}
	// %w, not %v: the underlying cause must survive the added context.
	if !strings.Contains(err.Error(), `OPA policy module "never-registered" is not registered`) {
		t.Fatalf("error = %q, want it to retain the underlying cause", err)
	}
}

func TestPolicyMiddlewarePanicsOnUncompilablePolicy(t *testing.T) {
	// Compilation moved out of Policy, so PolicyMiddleware is now the point
	// that fails fast during route construction.
	defer func() {
		recovered := recover()
		if recovered == nil {
			t.Fatal("PolicyMiddleware did not panic on invalid Rego")
		}
		if !strings.Contains(fmt.Sprint(recovered), "compile policy") {
			t.Fatalf("panic = %v, want it to mention compile policy", recovered)
		}
	}()

	PolicyMiddleware(Policy(
		Inline("broken.rego", "this is not valid rego at all"),
		Query("data.broken.allow"),
	))
}

func TestExplainRendersTree(t *testing.T) {
	// Registered sources need no registration here: Explain only calls
	// sourceKey(), never modules(), and a registered key is a plain string
	// with no digest in it, so the expected output is stable.
	node := All(
		Policy(Registered("a"), Query("data.a.allow")),
		Any(
			Policy(Registered("b"), Query("data.b.allow"), WithRawBody(1024)),
			Policy(Registered("c"), Query("data.c.allow")),
		),
	)

	// Compared whole rather than by substring: indentation, child order, and
	// the nesting of Any inside All are the behavior under test.
	want := strings.Join([]string{
		"All",
		"  Leaf query=data.a.allow source=registered:a",
		"  Any",
		"    Leaf query=data.b.allow source=registered:b body=1024",
		"    Leaf query=data.c.allow source=registered:c",
		"",
	}, "\n")

	if got := Explain(node); got != want {
		t.Fatalf("Explain output:\n%s\nwant:\n%s", got, want)
	}
}

func TestExplainRendersFactsAndUnstableSources(t *testing.T) {
	node := Policy(
		FromFS(fstest.MapFS{}, "p.rego"),
		Query("data.p.allow"),
		WithFacts("user", &testFactProvider{cacheKey: "user-facts"}),
	)

	// An fs source has no stable identity, so Explain falls back to the Go
	// type rather than inventing one.
	want := "Leaf query=data.p.allow source=auth.fsPolicySource facts=user@user-facts\n"
	if got := Explain(node); got != want {
		t.Fatalf("Explain output:\n%s\nwant:\n%s", got, want)
	}
}

func TestSourceKeyIdentity(t *testing.T) {
	// Registered and inline sources have stable identity; fs sources do not.
	if got := Registered("x").sourceKey(); got != "registered:x" {
		t.Fatalf("registered sourceKey = %q", got)
	}
	a := Inline("n.rego", "package n\n").sourceKey()
	b := Inline("n.rego", "package n\n").sourceKey()
	c := Inline("n.rego", "package n\nallow { true }\n").sourceKey()
	if a != b {
		t.Fatal("identical inline sources produced different keys")
	}
	if a == c {
		t.Fatal("inline sources with different bodies produced the same key")
	}
	if got := FromFS(fstest.MapFS{}, "p.rego").sourceKey(); got != "" {
		t.Fatalf("fs sourceKey = %q, want \"\" (no stable identity)", got)
	}
}

func TestMultiSourceKeyPoisonsOnUnstableChild(t *testing.T) {
	// One child without stable identity makes the whole multi unidentifiable.
	// Anything weaker would let a dedupe pass merge a multi whose fs child
	// might resolve to different Rego.
	stable := Modules(Registered("a"), Registered("b"))
	if stable.sourceKey() == "" {
		t.Fatal("multi of registered sources has no key, want a stable one")
	}
	poisoned := Modules(Registered("a"), FromFS(fstest.MapFS{}, "p.rego"))
	if got := poisoned.sourceKey(); got != "" {
		t.Fatalf("multi with fs child sourceKey = %q, want \"\"", got)
	}
}

func TestMultiSourceKeyIsOrderSensitive(t *testing.T) {
	// Module order is part of what gets compiled, so it must be part of the
	// identity.
	forward := Modules(Registered("a"), Registered("b")).sourceKey()
	reverse := Modules(Registered("b"), Registered("a")).sourceKey()
	if forward == reverse {
		t.Fatalf("Modules(a,b) and Modules(b,a) share key %q", forward)
	}
}

func TestLeafKeyIsInjective(t *testing.T) {
	// key() joins source, query, facts and body limit with separators, so every
	// component has to be quoted or a component containing a separator can be
	// mistaken for the boundary between two of them. A collision here is not a
	// cosmetic bug: Dedupe merges leaves with equal keys, so two policies that
	// check different things would become one, and the one that disappears is
	// never enforced again.
	//
	// Both pairs below are reachable through the public API. Registered takes
	// an arbitrary name, Query takes an arbitrary query, and WithFacts takes an
	// arbitrary namespace; none of them reject a separator.
	cases := []struct {
		name  string
		left  Leaf
		right Leaf
	}{
		{
			// Unquoted, both encode as leaf|registered:x|y|z||0.
			name:  "the boundary between source and query",
			left:  Policy(Registered("x"), Query("y|z")).(Leaf),
			right: Policy(Registered("x|y"), Query("z")).(Leaf),
		},
		{
			// Unquoted, both encode the facts field as a=b=c.
			name:  "the boundary between a fact namespace and its cache key",
			left:  Policy(Registered("s"), Query("q"), WithFacts("a", &testFactProvider{cacheKey: "b=c"})).(Leaf),
			right: Policy(Registered("s"), Query("q"), WithFacts("a=b", &testFactProvider{cacheKey: "c"})).(Leaf),
		},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			left, right := testCase.left.key(), testCase.right.key()
			if left == "" || right == "" {
				t.Fatalf("a leaf under test has no stable identity: %q and %q", left, right)
			}
			if left == right {
				t.Fatalf("two different leaves share key %q", left)
			}
		})
	}
}

func TestMultiSourceKeyIsInjective(t *testing.T) {
	// Children are quoted before joining, so a separator inside one child's
	// key cannot be confused for the boundary between two children.
	// Unquoted, both of these join to "registered:x,registered:y,registered:z".
	left := Modules(Registered("x,registered:y"), Registered("z")).sourceKey()
	right := Modules(Registered("x"), Registered("y,registered:z")).sourceKey()
	if left == right {
		t.Fatalf("multi keys collide across different groupings: %q", left)
	}
}
