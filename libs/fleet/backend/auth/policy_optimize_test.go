package auth

import (
	"fmt"
	"math/rand"
	"net/http"
	"strings"
	"testing"
	"testing/fstest"
)

// These tests pin what each pass does. The differential harness in
// policy_equivalence_test.go pins that the passes do not change what a policy
// decides — but it cannot tell an optimizer that works from one that returns
// its input, because a no-op preserves semantics perfectly. Everything below
// asserts the rewrites actually happen, and that they stop where they must.

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// The leaf builders every test in this package draws from. One per thing the
// cost model tiers, so a test can say what a leaf costs by which builder made
// it: allowLeaf and denyLeaf are costPure, bodyLeaf is costBody, factLeaf is
// costFacts.

func allowLeaf(name string) Node {
	return Policy(
		Inline(name+".rego", "package "+name+"\nallow { true }\n"),
		Query("data."+name+".allow"),
	)
}

func denyLeaf(name string) Node {
	return Policy(
		Inline(name+".rego", "package "+name+"\ndefault allow = false\n"),
		Query("data."+name+".allow"),
	)
}

// costBodyLimit is the raw-body limit bodyLeaf declares. Its own constant
// rather than a borrowed one: the harness's equivalenceWideBodyLimit is tied by
// a compile-time assertion to the length of the request body those tests send,
// and nothing here depends on that relationship. Any positive limit tiers a
// leaf as costBody.
const costBodyLimit = 1024

func bodyLeaf(name string) Node {
	return Policy(
		Inline(name+".rego", "package "+name+"\nallow { true }\n"),
		Query("data."+name+".allow"),
		WithRawBody(costBodyLimit),
	)
}

// factLeaf allows when its facts loaded, so it is cheap to reason about and
// expensive to evaluate — the shape sink-facts exists to move. Each provider
// gets its own namespace, but the Rego only reads the first: what the leaf
// decides is not the point, what it had to load to decide is.
func factLeaf(name string, providers ...FactProvider) Node {
	options := []PolicyOption{Query("data." + name + ".allow")}
	for index, provider := range providers {
		options = append(options, WithFacts(fmt.Sprintf("u%d", index), provider))
	}
	return Policy(
		Inline(name+".rego", "package "+name+"\nallow { input.facts.u0.ok }\n"),
		options...,
	)
}

// loadingFacts is the provider factLeaf is usually given: it succeeds, and
// testFactProvider counts the loads. That count is the whole point of the pass
// — a fact load is a database or API call, and sinking the leaf that needs it
// behind a cheap check is what removes the call rather than merely reordering
// it.
func loadingFacts(cacheKey string) *testFactProvider {
	return &testFactProvider{cacheKey: cacheKey, facts: FactSet{"ok": true}}
}

// dedupeFS is a filesystem whose one module is byte-identical to every other
// dedupeFS. Two of these are indistinguishable by path and by content, and
// still must not be merged: fs.FS has no identity a pass can compare.
func dedupeFS() fstest.MapFS {
	return fstest.MapFS{"p.rego": {Data: []byte("package p\nallow { true }\n")}}
}

// countingPolicySource counts how often its Rego is loaded. Compile loads each
// surviving leaf's source exactly once, which is what makes "did the middleware
// run the pipeline" observable: PolicyMiddleware's only other output is an HTTP
// status, and dropping a duplicate leaf does not change one.
type countingPolicySource struct {
	name  string
	text  string
	loads *int
}

func (countingPolicySource) policySource() {}

func (source countingPolicySource) modules() ([]policyModule, error) {
	*source.loads++
	return []policyModule{{name: source.name, source: source.text}}, nil
}

func (source countingPolicySource) sourceKey() string { return "counting:" + source.name }

func countingAllow(name string, loads *int) Node {
	return Policy(
		countingPolicySource{name: name + ".rego", text: "package " + name + "\nallow { true }\n", loads: loads},
		Query("data."+name+".allow"),
	)
}

// registerTestPass installs a pass for the duration of one test. It refuses to
// shadow a real pass, so a test cannot quietly replace flatten with a recorder
// and then assert about flatten.
func registerTestPass(t *testing.T, name string, pass optimizer) {
	t.Helper()
	if _, exists := optimizerRegistry[name]; exists {
		t.Fatalf("pass %q is already registered; pick a name that is not a real pass", name)
	}
	optimizerRegistry[name] = pass
	t.Cleanup(func() { delete(optimizerRegistry, name) })
}

// emptyCompositeIn finds a childless All or Any, the shape compileChildren
// rejects by name. Before that guard existed an empty conjunction compiled to
// an unconditional allow, so a pass that produced one would turn a startup
// error into a 200 if the guard were ever relaxed.
func emptyCompositeIn(n Node) Node {
	var children []Node
	switch node := n.(type) {
	case AllNode:
		children = node.Children
	case AnyNode:
		children = node.Children
	default:
		return nil
	}
	if len(children) == 0 {
		return n
	}
	for _, child := range children {
		if found := emptyCompositeIn(child); found != nil {
			return found
		}
	}
	return nil
}

func childrenOf(t *testing.T, n Node) []Node {
	t.Helper()
	switch node := n.(type) {
	case AllNode:
		return node.Children
	case AnyNode:
		return node.Children
	default:
		t.Fatalf("node is %T, want a composite\n%s", n, Explain(n))
		return nil
	}
}

// ---------------------------------------------------------------------------
// Flatten
// ---------------------------------------------------------------------------

func TestFlattenCollapsesNestedSameKindNodes(t *testing.T) {
	node := All(allowLeaf("a"), All(allowLeaf("b"), allowLeaf("c")))
	got := Optimize(node, []string{"flatten"})

	// Compared whole: the count alone would not notice a pass that merged the
	// right number of children in the wrong order or lost one to a duplicate.
	want := Explain(All(allowLeaf("a"), allowLeaf("b"), allowLeaf("c")))
	if Explain(got) != want {
		t.Fatalf("Explain output:\n%s\nwant:\n%s", Explain(got), want)
	}
}

func TestFlattenMergesRecursively(t *testing.T) {
	// mapChildren rewrites bottom-up, so a chain deeper than one level must
	// collapse in a single application of the pass.
	node := All(allowLeaf("a"), All(allowLeaf("b"), All(allowLeaf("c"), allowLeaf("d"))))
	got := flatten(node)
	if children := childrenOf(t, got); len(children) != 4 {
		t.Fatalf("children = %d, want 4\n%s", len(children), Explain(got))
	}
}

func TestFlattenLeavesMixedKindsAlone(t *testing.T) {
	// Only associativity within one combinator is available: min distributes
	// over nothing here, so All must not absorb an Any's children.
	node := All(allowLeaf("a"), Any(allowLeaf("b"), allowLeaf("c")))
	got := Optimize(node, []string{"flatten"})

	children := childrenOf(t, got)
	if len(children) != 2 {
		t.Fatalf("children = %d, want 2\n%s", len(children), Explain(got))
	}
	if _, ok := children[1].(AnyNode); !ok {
		t.Fatalf("second child = %T, want AnyNode\n%s", children[1], Explain(got))
	}

	// The mirror image, so that a pass hard-coded to one kind is caught too.
	mirrored := Optimize(Any(allowLeaf("a"), All(allowLeaf("b"), allowLeaf("c"))), []string{"flatten"})
	mirroredChildren := childrenOf(t, mirrored)
	if len(mirroredChildren) != 2 {
		t.Fatalf("mirrored children = %d, want 2\n%s", len(mirroredChildren), Explain(mirrored))
	}
	if _, ok := mirroredChildren[1].(AllNode); !ok {
		t.Fatalf("mirrored second child = %T, want AllNode\n%s", mirroredChildren[1], Explain(mirrored))
	}
}

func TestFlattenLeavesChildlessCompositesForCompileToReject(t *testing.T) {
	// A childless composite is malformed input, not something to absorb.
	// Absorbing its zero children would erase the evidence, and for
	// All(AllNode{}) it would leave the parent childless as well.
	cases := []struct {
		name string
		node Node
	}{
		{"empty inner conjunction", AllNode{Children: []Node{allowLeaf("a"), AllNode{}}}},
		{"empty sole child", AllNode{Children: []Node{AllNode{}}}},
		{"empty inner disjunction", AnyNode{Children: []Node{allowLeaf("a"), AnyNode{}}}},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			got := flatten(testCase.node)
			// Compared whole rather than asking whether an empty composite
			// survives somewhere. The first draft asked the weaker question,
			// and for "empty sole child" splicing merely moves the childless
			// node up to the root — where emptyCompositeIn still finds it, so
			// that subcase detected nothing. "Unchanged" is what the guard
			// actually promises, and it is the same promise for all three.
			if Explain(got) != Explain(testCase.node) {
				t.Fatalf("Flatten rewrote a tree containing a childless composite:\n%s\nwant it untouched:\n%s",
					Explain(got), Explain(testCase.node))
			}
			if emptyCompositeIn(got) == nil {
				t.Fatalf("Flatten swallowed the childless composite:\n%s", Explain(got))
			}
			if _, err := Compile(got); err == nil {
				t.Fatalf("Compile accepted a tree with a childless composite:\n%s", Explain(got))
			}
		})
	}
}

// ---------------------------------------------------------------------------
// CollapseUnary
// ---------------------------------------------------------------------------

func TestCollapseUnaryRemovesSingleChildNodes(t *testing.T) {
	node := All(Any(allowLeaf("a")))
	got := Optimize(node, []string{"collapse"})
	if _, ok := got.(Leaf); !ok {
		t.Fatalf("got %T, want Leaf\n%s", got, Explain(got))
	}
}

func TestCollapseUnaryKeepsMultiChildNodes(t *testing.T) {
	// A fold over one element is that element; a fold over two is not either of
	// them. Both kinds, so a pass that checked the wrong length for one of them
	// is caught.
	cases := []struct {
		name string
		node Node
	}{
		{"conjunction", All(allowLeaf("a"), allowLeaf("b"))},
		{"disjunction", Any(allowLeaf("a"), allowLeaf("b"))},
		{"nested", All(allowLeaf("a"), Any(allowLeaf("b"), allowLeaf("c")))},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			got := Optimize(testCase.node, []string{"collapse"})
			if Explain(got) != Explain(testCase.node) {
				t.Fatalf("collapse rewrote a multi-child node:\n%s\nwant:\n%s", Explain(got), Explain(testCase.node))
			}
		})
	}
}

// ---------------------------------------------------------------------------
// Dedupe
// ---------------------------------------------------------------------------

func TestDedupeRemovesIdenticalLeaves(t *testing.T) {
	node := All(allowLeaf("a"), allowLeaf("b"), allowLeaf("a"))
	got := Optimize(node, []string{"dedupe"})

	// The whole rendering, not the child count: dropping the duplicate and
	// dropping "b" both leave two children.
	want := Explain(All(allowLeaf("a"), allowLeaf("b")))
	if Explain(got) != want {
		t.Fatalf("Explain output:\n%s\nwant:\n%s", Explain(got), want)
	}
}

func TestDedupeAppliesInsideNestedNodes(t *testing.T) {
	node := All(allowLeaf("a"), Any(allowLeaf("b"), allowLeaf("b")))
	got := Optimize(node, []string{"dedupe"})

	children := childrenOf(t, got)
	if len(children) != 2 {
		t.Fatalf("outer children = %d, want 2\n%s", len(children), Explain(got))
	}
	if inner := childrenOf(t, children[1]); len(inner) != 1 {
		t.Fatalf("inner children = %d, want 1\n%s", len(inner), Explain(got))
	}
}

func TestDedupeKeepsLeavesThatAreNotProvablyIdentical(t *testing.T) {
	// Every component of Leaf.key() gets a pair that differs in it alone. A
	// dedupe keyed on any subset of them would merge one of these pairs, which
	// silently removes an authorization check.
	facts := &testFactProvider{cacheKey: "facts-a", facts: FactSet{"ok": true}}
	otherFacts := &testFactProvider{cacheKey: "facts-b", facts: FactSet{"ok": true}}
	source := Inline("p.rego", "package p\nallow { true }\n")

	cases := []struct {
		name string
		node Node
	}{
		{
			name: "different queries",
			node: All(Policy(source, Query("data.p.allow")), Policy(source, Query("data.p.other"))),
		},
		{
			name: "different sources",
			node: All(allowLeaf("a"), allowLeaf("b")),
		},
		{
			name: "same source name, different text",
			node: All(
				Policy(Inline("p.rego", "package p\nallow { true }\n"), Query("data.p.allow")),
				Policy(Inline("p.rego", "package p\nallow { false }\n"), Query("data.p.allow")),
			),
		},
		{
			name: "different facts",
			node: All(
				Policy(source, Query("data.p.allow"), WithFacts("u", facts)),
				Policy(source, Query("data.p.allow"), WithFacts("u", otherFacts)),
			),
		},
		{
			name: "different fact namespaces",
			node: All(
				Policy(source, Query("data.p.allow"), WithFacts("u", facts)),
				Policy(source, Query("data.p.allow"), WithFacts("v", facts)),
			),
		},
		{
			// A separator inside a component, which an unquoted encoding would
			// let cross a field boundary: both of these encode as
			// leaf|registered:x|y|z||0 unless key() quotes what it joins. See
			// TestLeafKeyIsInjective.
			name: "a source and query that collide unless the key is quoted",
			node: All(
				Policy(Registered("x"), Query("y|z")),
				Policy(Registered("x|y"), Query("z")),
			),
		},
		{
			name: "different body limits",
			node: All(
				Policy(source, Query("data.p.allow"), WithRawBody(16)),
				Policy(source, Query("data.p.allow"), WithRawBody(32)),
			),
		},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			got := Optimize(testCase.node, []string{"dedupe"})
			if children := childrenOf(t, got); len(children) != 2 {
				t.Fatalf("children = %d, want 2 (these leaves are not identical)\n%s", len(children), Explain(got))
			}
		})
	}
}

func TestDedupeNeverMergesLeavesWithoutStableIdentity(t *testing.T) {
	// key() returns "" for a source with no stable identity, and "" means
	// "never equal to anything, including itself". fs.FS is the case that
	// matters: two filesystems can share a path and serve different Rego, so
	// merging them would swap one policy for another with no visible change.
	//
	// The generated trees in the differential harness are all Inline, so this
	// rule is exercised nowhere else.
	shared := FromFS(dedupeFS(), "p.rego")

	cases := []struct {
		name string
		node Node
		want int
	}{
		{
			name: "the very same fs source twice",
			node: All(Policy(shared, Query("data.p.allow")), Policy(shared, Query("data.p.allow"))),
			want: 2,
		},
		{
			name: "two filesystems with identical contents at the same path",
			node: All(
				Policy(FromFS(dedupeFS(), "p.rego"), Query("data.p.allow")),
				Policy(FromFS(dedupeFS(), "p.rego"), Query("data.p.allow")),
			),
			want: 2,
		},
		{
			name: "a multi source poisoned by an fs child",
			node: All(
				Policy(Modules(Registered("r"), shared), Query("data.p.allow")),
				Policy(Modules(Registered("r"), shared), Query("data.p.allow")),
			),
			want: 2,
		},
		{
			// The control. Without it, a Dedupe that merged nothing at all would
			// pass every case above.
			name: "identical inline leaves still merge",
			node: All(allowLeaf("a"), allowLeaf("a")),
			want: 1,
		},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			got := Optimize(testCase.node, []string{"dedupe"})
			if children := childrenOf(t, got); len(children) != testCase.want {
				t.Fatalf("children = %d, want %d\n%s", len(children), testCase.want, Explain(got))
			}
		})
	}
}

func TestDedupeLeavesOneChildBehind(t *testing.T) {
	// Dedupe keeps the first occurrence of every key, so it cannot empty a
	// composite. Asserted rather than assumed: a childless conjunction is the
	// one rewrite outcome that fails as a startup error instead of a wrong
	// answer, and only because compileChildren rejects it.
	node := All(allowLeaf("a"), allowLeaf("a"), allowLeaf("a"))
	got := Optimize(node, []string{"dedupe"})

	if empty := emptyCompositeIn(got); empty != nil {
		t.Fatalf("dedupe produced a childless composite:\n%s", Explain(got))
	}
	if children := childrenOf(t, got); len(children) != 1 {
		t.Fatalf("children = %d, want 1\n%s", len(children), Explain(got))
	}
	if _, err := Compile(got); err != nil {
		t.Fatalf("Compile: %v\n%s", err, Explain(got))
	}
}

// ---------------------------------------------------------------------------
// The registry and the fixpoint loop
// ---------------------------------------------------------------------------

func TestOptimizeRunsPassesInPipelineOrder(t *testing.T) {
	var calls []string
	recorder := func(name string) optimizer {
		return func(n Node) Node {
			calls = append(calls, name)
			return n
		}
	}
	registerTestPass(t, "test-record-1", recorder("test-record-1"))
	registerTestPass(t, "test-record-2", recorder("test-record-2"))
	registerTestPass(t, "test-record-3", recorder("test-record-3"))

	// Deliberately not a palindrome: a first draft used one, and reversing the
	// loop that runs the passes then changed nothing this test could see.
	// A name appears twice as well, because a pipeline is a list, not a set.
	Optimize(allowLeaf("a"), []string{"test-record-2", "test-record-1", "test-record-1", "test-record-3"})

	// Exactly one iteration, because nothing changed. Both halves matter: the
	// order proves the pipeline is a sequence rather than a set, and the count
	// proves a converged tree is not re-optimized eight times.
	want := "test-record-2 test-record-1 test-record-1 test-record-3"
	if got := strings.Join(calls, " "); got != want {
		t.Fatalf("passes ran as %q, want %q", got, want)
	}
}

func TestOptimizeRepeatsUntilFixpoint(t *testing.T) {
	// collapse turns Any(All(b, c)) into All(b, c), which hands flatten a merge
	// it could not see on the first sweep. One pipeline sweep is therefore not
	// enough, and the assertion below spells out what one sweep would leave.
	node := All(allowLeaf("a"), Any(All(allowLeaf("b"), allowLeaf("c"))))

	oneSweep := dedupe(collapseUnary(flatten(node)))
	if children := childrenOf(t, oneSweep); len(children) != 2 {
		t.Fatalf("a single sweep gave %d children, want 2; the test no longer tests iteration\n%s",
			len(children), Explain(oneSweep))
	}

	got := Optimize(node, DefaultPipeline())
	if children := childrenOf(t, got); len(children) != 3 {
		t.Fatalf("children = %d, want 3 (a, b, c)\n%s", len(children), Explain(got))
	}
}

func TestOptimizeStopsAtTheIterationCap(t *testing.T) {
	// A pass that never converges must not hang route construction. Wrapping
	// the tree in a fresh node changes Explain every time, so only the cap can
	// end this.
	runs := 0
	registerTestPass(t, "test-grow", func(n Node) Node {
		runs++
		return AllNode{Children: []Node{n}}
	})

	got := Optimize(allowLeaf("a"), []string{"test-grow"})

	if runs != optimizeFixpointLimit {
		t.Fatalf("pass ran %d times, want the cap of %d", runs, optimizeFixpointLimit)
	}
	depth := 0
	for current := got; ; depth++ {
		node, ok := current.(AllNode)
		if !ok {
			break
		}
		current = node.Children[0]
	}
	if depth != optimizeFixpointLimit {
		t.Fatalf("result is %d levels deep, want %d", depth, optimizeFixpointLimit)
	}
}

func TestOptimizeIsIdempotent(t *testing.T) {
	node := All(
		allowLeaf("a"),
		All(allowLeaf("b"), allowLeaf("a")),
		Any(allowLeaf("c")),
	)
	once := Optimize(node, DefaultPipeline())
	twice := Optimize(once, DefaultPipeline())

	if Explain(once) != Explain(twice) {
		t.Fatalf("pipeline is not idempotent:\n--- once ---\n%s\n--- twice ---\n%s", Explain(once), Explain(twice))
	}
	// Idempotence is satisfied by an optimizer that does nothing, so pin that
	// this tree really is rewritten.
	if Explain(once) == Explain(node) {
		t.Fatalf("pipeline left a reducible tree untouched:\n%s", Explain(node))
	}
}

func TestOptimizeWithAnEmptyPipelineIsTheIdentity(t *testing.T) {
	node := All(allowLeaf("a"), All(allowLeaf("b"), allowLeaf("a")))
	if got := Optimize(node, nil); Explain(got) != Explain(node) {
		t.Fatalf("empty pipeline rewrote the tree:\n%s\nwant:\n%s", Explain(got), Explain(node))
	}

	// Returning the tree unchanged is not the same as doing nothing: without
	// the early return, an empty pipeline still renders the whole tree twice to
	// discover that no pass changed it. Nothing about the result can show that,
	// so the work itself is what gets measured.
	//
	// The threshold is loose on purpose — the claim is "no tree-sized work",
	// not an exact allocation count. Rendering this tree costs well over a
	// dozen allocations, so a mutation that drops the early return cannot slip
	// under it, and ordinary churn cannot trip it.
	if allocs := testing.AllocsPerRun(50, func() { Optimize(node, nil) }); allocs >= 3 {
		t.Fatalf("a disabled pipeline made %v allocations, want it to do no work", allocs)
	}
}

func TestOptimizePanicsOnUnknownPassName(t *testing.T) {
	// Everything else in this package fails loudly at route construction. A
	// mistyped pass name that merely skipped the pass would ship a route
	// running a pipeline nobody asked for, and nothing downstream would notice.
	defer func() {
		recovered := recover()
		if recovered == nil {
			t.Fatal("Optimize accepted an unknown pass name")
		}
		if !strings.Contains(fmt.Sprint(recovered), "dedupe-typo") {
			t.Fatalf("panic = %v, want it to name the unknown pass", recovered)
		}
	}()

	Optimize(allowLeaf("a"), []string{"flatten", "dedupe-typo"})
}

func TestOptimizeValidatesEveryNameBeforeRewriting(t *testing.T) {
	// Resolution happens up front, so a bad name late in the pipeline aborts
	// before any earlier pass has run. Not because a pass would corrupt the
	// tree — the passes here are pure, and a rewritten tree from an aborted
	// pipeline is simply discarded — but because a pass is free to have side
	// effects of its own, and route construction should not half-run a
	// pipeline it is about to reject.
	ran := false
	registerTestPass(t, "test-should-not-run", func(n Node) Node {
		ran = true
		return n
	})

	func() {
		defer func() { _ = recover() }()
		Optimize(allowLeaf("a"), []string{"test-should-not-run", "no-such-pass"})
	}()

	if ran {
		t.Fatal("a pass ran before the pipeline was validated")
	}
}

func TestDefaultPipelineNamesRegisteredPasses(t *testing.T) {
	for _, name := range DefaultPipeline() {
		if _, ok := optimizerRegistry[name]; !ok {
			t.Errorf("DefaultPipeline names %q, which is not in the registry", name)
		}
	}
}

func TestDefaultPipelineHandsOutAFreshSlice(t *testing.T) {
	// The pipeline decides which rewrites every route's plan receives. If
	// callers shared one slice, anything holding it could rewrite the default
	// for every route built afterwards, and nothing in this package checks a
	// pipeline against what someone meant to configure.
	// Joined into a string, not held as a slice. A first draft kept the
	// original as []string, which aliases the shared slice under exactly the
	// bug this test exists to find — so it compared a value with itself and
	// could not fail.
	original := strings.Join(DefaultPipeline(), ",")
	if original == "" {
		t.Fatal("DefaultPipeline is empty")
	}

	tampered := DefaultPipeline()
	for index := range tampered {
		tampered[index] = "no-such-pass"
	}

	if after := strings.Join(DefaultPipeline(), ","); after != original {
		t.Fatalf("DefaultPipeline is now %q, want %q", after, original)
	}
}

func TestStructuralPassesCommuteToTheSameFixpoint(t *testing.T) {
	// Documented rather than assumed: over flatten, collapse and dedupe the
	// order within a pipeline does not change the fixpoint, because each pass
	// only ever exposes work for the others. This is why reordering these three
	// names is not observable in the output tree — order starts to matter with
	// sink-facts, whose output is not a normal form.
	node := All(
		allowLeaf("a"),
		All(allowLeaf("b"), allowLeaf("a")),
		Any(All(allowLeaf("c"), allowLeaf("c"))),
	)

	names := []string{"flatten", "collapse", "dedupe"}
	want := Explain(Optimize(node, names))
	for _, permuted := range permutations(names) {
		if got := Explain(Optimize(node, permuted)); got != want {
			t.Fatalf("pipeline %v converged to:\n%s\nbut %v converged to:\n%s", permuted, got, names, want)
		}
	}
	if want == Explain(node) {
		t.Fatalf("the tree under test is already in normal form:\n%s", Explain(node))
	}
}

// ---------------------------------------------------------------------------
// The pipeline over generated trees
// ---------------------------------------------------------------------------

func TestDefaultPipelineRewritesGeneratedTrees(t *testing.T) {
	// TestOptimizerPreservesSemantics runs the pipeline over these same trees
	// and would be perfectly green against an optimizer that returned its
	// input. This is the floor that says the optimizer does something, and that
	// what it produces is still a compilable tree.
	//
	// The floor is a third of the range; measured, the pipeline rewrites 294 of
	// 300, so ordinary churn in the generator will not trip it but an optimizer
	// that quietly stopped rewriting will.
	const wantAtLeast = equivalenceSeeds / 3

	rewritten := 0
	for seed := int64(1); seed <= equivalenceSeeds; seed++ {
		tree := genTree(rand.New(rand.NewSource(seed)), equivalenceDepth)
		node := tree.astNode()
		optimized := Optimize(node, DefaultPipeline())

		if Explain(optimized) != Explain(node) {
			rewritten++
		}
		// Optimize stops after optimizeFixpointLimit sweeps whether or not the
		// tree has settled, and a tree that needed more comes back correct,
		// partly optimized, and silent about it. Nothing else would notice:
		// TestOptimizerPreservesSemantics is green either way, because a
		// partly-optimized plan decides exactly what a finished one decides.
		//
		// Re-running the pipeline on the result must therefore change nothing.
		// The corpus needs at most 3 sweeps today — measured directly by
		// TestDefaultPipelineConvergesWellInsideTheSweepCap below, and unchanged
		// by sink-facts, which sorts within a node and so exposes no new merges
		// for the other three passes.
		if again := Optimize(optimized, DefaultPipeline()); Explain(again) != Explain(optimized) {
			t.Fatalf("seed %d: not a fixpoint after %d sweeps; another pipeline run changed it\n--- after Optimize ---\n%s\n--- after a second Optimize ---\n%s",
				seed, optimizeFixpointLimit, Explain(optimized), Explain(again))
		}
		if empty := emptyCompositeIn(optimized); empty != nil {
			t.Fatalf("seed %d: pipeline produced a childless %T\n%s", seed, empty, Explain(optimized))
		}
		if _, err := Compile(optimized); err != nil {
			t.Fatalf("seed %d: optimized tree does not compile: %v\n%s", seed, err, Explain(optimized))
		}
	}

	t.Logf("pipeline rewrote %d of %d generated trees", rewritten, equivalenceSeeds)
	if rewritten < wantAtLeast {
		t.Errorf("pipeline rewrote %d of %d trees, want at least %d", rewritten, equivalenceSeeds, wantAtLeast)
	}
}

func TestDefaultPipelineConvergesWellInsideTheSweepCap(t *testing.T) {
	// Optimize stops after optimizeFixpointLimit sweeps whether or not the tree
	// has settled, and a tree that needed more comes back correct, partly
	// optimized, and silent about it. The fixpoint assertion above catches the
	// moment that starts happening; this reports how much room is left before it
	// does, so the headroom is a number in a log rather than an assumption.
	//
	// The count comes from a pass appended to the real pipeline, not from a
	// reimplementation of the loop here: a copy of Optimize's loop could drift
	// from it and would then measure the wrong thing.
	//
	// One pass registered once and reset per seed, rather than one per seed.
	// registerTestPass is the only way a test may touch the registry — it
	// refuses to shadow a real pass and unregisters through t.Cleanup, so a
	// panic inside Optimize cannot leak an entry into every later test in the
	// package — but three hundred registrations would queue three hundred
	// cleanups to say the same thing. A counter the loop resets says it once.
	const wantHeadroom = 2

	sweeps := 0
	registerTestPass(t, "test-sweep-count", func(n Node) Node {
		sweeps++
		return n
	})
	counted := append(DefaultPipeline(), "test-sweep-count")

	worst, worstSeed := 0, int64(0)
	for seed := int64(1); seed <= equivalenceSeeds; seed++ {
		node := genTree(rand.New(rand.NewSource(seed)), equivalenceDepth).astNode()

		sweeps = 0
		Optimize(node, counted)

		if sweeps > worst {
			worst, worstSeed = sweeps, seed
		}
	}

	t.Logf("worst case %d sweeps (seed %d) against a cap of %d", worst, worstSeed, optimizeFixpointLimit)
	if worst > optimizeFixpointLimit-wantHeadroom {
		t.Errorf("worst case %d sweeps (seed %d) leaves less than %d spare under the cap of %d; raise the cap deliberately or find out why a pass stopped converging",
			worst, worstSeed, wantHeadroom, optimizeFixpointLimit)
	}
}

func TestOptimizeLeavesTheCallersTreeAlone(t *testing.T) {
	// Every pass builds new composites and returns leaves untouched, so
	// Optimize is a pure function of its input and one AST may safely be shared
	// between routes — each getting its own plan from it, and a later
	// PolicyMiddleware seeing what its caller wrote rather than what an earlier
	// one left behind.
	//
	// That was a property of the code and of nothing else until now. It is easy
	// to lose by accident: a pass that sorted or spliced a child slice in place
	// would reach through the Node value into the caller's backing array, and
	// every other test here reads only the returned tree, so all of them would
	// stay green. sink-facts is the first pass that reorders, which is what
	// makes an in-place slip possible at all.
	//
	// Each pass alone, and then the whole pipeline. Running only the pipeline
	// is not enough, and measurably so: with sink-facts mutated to sort the
	// caller's slice in place, the pipeline case still passed. flatten runs
	// first and rebuilds every composite, so by the time sink-facts sees the
	// tree it is holding copies and can no longer reach what the caller wrote.
	// A pipeline test therefore proves the property for whichever pass happens
	// to run first and asserts nothing about the rest.
	pipelines := [][]string{}
	for _, name := range DefaultPipeline() {
		pipelines = append(pipelines, []string{name})
	}
	pipelines = append(pipelines, DefaultPipeline())

	for _, pipeline := range pipelines {
		label := strings.Join(pipeline, ",")
		for seed := int64(1); seed <= equivalenceSeeds; seed++ {
			// A fresh tree per run: a pass that did mutate its input would
			// otherwise hand the next pipeline a tree already rewritten, and the
			// failure would be reported against the wrong pass.
			node := genTree(rand.New(rand.NewSource(seed)), equivalenceDepth).astNode()

			before := Explain(node)
			Optimize(node, pipeline)
			if after := Explain(node); after != before {
				t.Fatalf("seed %d: pipeline %q mutated the tree it was given\n--- before ---\n%s\n--- after ---\n%s",
					seed, label, before, after)
			}
		}
	}
}

// ---------------------------------------------------------------------------
// Middleware wiring
// ---------------------------------------------------------------------------

func TestWithPipelineCopiesItsNames(t *testing.T) {
	// Spreading a slice into a variadic parameter shares the backing array, so
	// without a copy the config would hold a live view of the caller's slice.
	names := []string{"flatten", "collapse"}
	config := middlewareConfig{}
	WithPipeline(names...)(&config)
	names[0] = "no-such-pass"

	if got := strings.Join(config.pipeline, ","); got != "flatten,collapse" {
		t.Fatalf("stored pipeline = %q, want %q", got, "flatten,collapse")
	}
}

func TestPolicyMiddlewareOptimizesByDefault(t *testing.T) {
	// Compile loads each surviving leaf's source once, so the load count is a
	// direct readout of how many leaves the plan kept.
	loads := 0
	node := All(countingAllow("a", &loads), countingAllow("a", &loads))

	if status := runPolicyStatus(t, node); status != http.StatusNoContent {
		t.Fatalf("status = %d, want %d", status, http.StatusNoContent)
	}
	if loads != 1 {
		t.Fatalf("compiled %d leaves, want 1: the default pipeline did not dedupe", loads)
	}
}

func TestPolicyMiddlewareWithPipelineDisablesOptimization(t *testing.T) {
	loads := 0
	node := All(countingAllow("a", &loads), countingAllow("a", &loads))

	if status := runPolicyStatus(t, node, WithPipeline()); status != http.StatusNoContent {
		t.Fatalf("status = %d, want %d", status, http.StatusNoContent)
	}
	if loads != 2 {
		t.Fatalf("compiled %d leaves, want 2: WithPipeline() must disable optimization", loads)
	}
}

func TestPolicyMiddlewareRunsTheNamedPipeline(t *testing.T) {
	loads := 0
	node := All(countingAllow("a", &loads), countingAllow("a", &loads))

	if status := runPolicyStatus(t, node, WithPipeline("flatten")); status != http.StatusNoContent {
		t.Fatalf("status = %d, want %d", status, http.StatusNoContent)
	}
	if loads != 2 {
		t.Fatalf("compiled %d leaves, want 2: a flatten-only pipeline must not dedupe", loads)
	}
}

func TestPolicyMiddlewarePanicsOnUnknownPipelineName(t *testing.T) {
	defer func() {
		recovered := recover()
		if recovered == nil {
			t.Fatal("PolicyMiddleware accepted an unknown pass name")
		}
		if !strings.Contains(fmt.Sprint(recovered), "no-such-pass") {
			t.Fatalf("panic = %v, want it to name the unknown pass", recovered)
		}
	}()

	PolicyMiddleware(allowLeaf("a"), WithPipeline("no-such-pass"))
}
