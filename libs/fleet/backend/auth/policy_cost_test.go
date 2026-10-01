package auth

import (
	"context"
	"fmt"
	"math/rand"
	"net/http"
	"slices"
	"strings"
	"testing"
)

// These tests measure the cost model and the pass that sorts by it.
//
// The differential harness in policy_equivalence_test.go cannot help here. It
// proves sinkFactLoaders does not change what a policy decides, and a pass that
// returned its input would satisfy that perfectly — measured with every pass
// stubbed out to the identity, TestOptimizerPreservesSemantics was green across
// all 300 seeds. Soundness and effectiveness are separate questions and only
// the tests below ask the second one.
//
// The leaf builders — allowLeaf, denyLeaf, bodyLeaf, factLeaf, loadingFacts —
// live in policy_optimize_test.go's shared helpers block.

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// pricyProvider declares itself dearer than the default through the optional
// CostReporter interface.
type pricyProvider struct{ key string }

func (provider pricyProvider) CacheKey() string { return provider.key }
func (pricyProvider) FactCost() int             { return 50 }
func (pricyProvider) LoadFacts(context.Context, *http.Request) (FactSet, error) {
	return FactSet{"ok": true}, nil
}

func leafOf(t *testing.T, n Node) Leaf {
	t.Helper()
	leaf, ok := n.(Leaf)
	if !ok {
		t.Fatalf("node is %T, want a Leaf", n)
	}
	return leaf
}

// queryOrder returns the leaf queries in the order Explain renders them, which
// for a tree of composites is the order lazy evaluation visits them.
func queryOrder(n Node) []string {
	var queries []string
	for _, line := range strings.Split(Explain(n), "\n") {
		_, rest, found := strings.Cut(line, "Leaf query=")
		if !found {
			continue
		}
		query, _, _ := strings.Cut(rest, " ")
		queries = append(queries, query)
	}
	return queries
}

// leafMultiset renders every leaf in a tree and sorts the renderings, so two
// trees compare equal exactly when they hold the same leaves with the same
// multiplicities, whatever the order or the nesting.
func leafMultiset(n Node) []string {
	var leaves []string
	for _, line := range strings.Split(Explain(n), "\n") {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "Leaf ") {
			leaves = append(leaves, trimmed)
		}
	}
	slices.Sort(leaves)
	return leaves
}

// ---------------------------------------------------------------------------
// The cost model
// ---------------------------------------------------------------------------

func TestStaticTiersClassifiesLeavesByWhatTheyNeed(t *testing.T) {
	pure := staticTiers{}.leafCost(leafOf(t, allowLeaf("p")))
	body := staticTiers{}.leafCost(leafOf(t, bodyLeaf("b")))
	facts := staticTiers{}.leafCost(leafOf(t, factLeaf("f", loadingFacts("tier"))))

	if pure.class != costPure {
		t.Errorf("a leaf reading only request attributes = class %d, want costPure (%d)", pure.class, costPure)
	}
	if body.class != costBody {
		t.Errorf("a WithRawBody leaf = class %d, want costBody (%d)", body.class, costBody)
	}
	if facts.class != costFacts {
		t.Errorf("a WithFacts leaf = class %d, want costFacts (%d)", facts.class, costFacts)
	}

	// The ordering itself, not just the labels. costLess compares classes with
	// <, so a permuted const block would keep every assertion above true while
	// inverting what the pass calls cheap.
	if !costLess(pure, body) {
		t.Errorf("pure %+v is not cheaper than body %+v", pure, body)
	}
	if !costLess(body, facts) {
		t.Errorf("body %+v is not cheaper than facts %+v", body, facts)
	}
	if !costLess(pure, facts) {
		t.Errorf("pure %+v is not cheaper than facts %+v", pure, facts)
	}
}

func TestStaticTiersClassifiesAFactsAndBodyLeafAsFacts(t *testing.T) {
	// A leaf that does both is priced by the dearer of the two. Checking the
	// body limit first would hide a database call behind it.
	both := Policy(
		Inline("both.rego", "package both\nallow { input.facts.u0.ok }\n"),
		Query("data.both.allow"),
		WithFacts("u0", loadingFacts("both")),
		WithRawBody(costBodyLimit),
	)
	if got := (staticTiers{}).leafCost(leafOf(t, both)); got.class != costFacts {
		t.Fatalf("a leaf with facts and a body limit = class %d, want costFacts (%d)", got.class, costFacts)
	}
}

func TestStaticTiersWeighsDistinctCacheKeysOnly(t *testing.T) {
	// Fact results are cached per request across the whole tree, so a second
	// consumer of a key already loaded costs nothing. Counting providers
	// instead of keys would price the two leaves below the same, and would make
	// a leaf that reuses one provider look as dear as one that loads two.
	shared := loadingFacts("shared")
	other := loadingFacts("other")

	repeated := staticTiers{}.leafCost(leafOf(t, factLeaf("repeat", shared, shared)))
	distinct := staticTiers{}.leafCost(leafOf(t, factLeaf("distinct", shared, other)))
	single := staticTiers{}.leafCost(leafOf(t, factLeaf("single", shared)))

	if repeated.weight != 1 {
		t.Errorf("two providers sharing a cache key weigh %d, want 1", repeated.weight)
	}
	if distinct.weight != 2 {
		t.Errorf("two providers with distinct cache keys weigh %d, want 2", distinct.weight)
	}
	if repeated.weight != single.weight {
		t.Errorf("reusing a provider changed the weight: %d vs %d", repeated.weight, single.weight)
	}
	if !costLess(repeated, distinct) {
		t.Errorf("one cache key %+v is not cheaper than two %+v", repeated, distinct)
	}

	// The first provider listed for a key is the one charged, so when two
	// different provider values share a cache key and disagree about their cost,
	// the weight follows declaration order. Sharing a key asserts they are the
	// same load, so disagreeing about its cost is a contradiction in the input —
	// but the model still has to answer, and this is the answer. Pinned so that
	// changing which one wins is a decision someone makes on purpose.
	plainFirst := staticTiers{}.leafCost(leafOf(t, factLeaf("plainfirst",
		&testFactProvider{cacheKey: "collide", facts: FactSet{"ok": true}},
		pricyProvider{key: "collide"})))
	reporterFirst := staticTiers{}.leafCost(leafOf(t, factLeaf("reporterfirst",
		pricyProvider{key: "collide"},
		&testFactProvider{cacheKey: "collide", facts: FactSet{"ok": true}})))

	if plainFirst.weight != 1 {
		t.Errorf("plain provider declared first weighs %d, want 1 (the first provider for a key is charged)", plainFirst.weight)
	}
	if reporterFirst.weight != 50 {
		t.Errorf("reporting provider declared first weighs %d, want 50 (the first provider for a key is charged)", reporterFirst.weight)
	}
}

func TestStaticTiersHonorsOptionalCostReporter(t *testing.T) {
	cheap := staticTiers{}.leafCost(leafOf(t, factLeaf("c", loadingFacts("cheap"))))
	pricey := staticTiers{}.leafCost(leafOf(t, factLeaf("d", pricyProvider{key: "pricy"})))

	if !costLess(cheap, pricey) {
		t.Fatalf("weights = %d, %d; a CostReporter provider should outweigh a default one", cheap.weight, pricey.weight)
	}
	if pricey.weight != 50 {
		t.Errorf("declared weight = %d, want the reported 50", pricey.weight)
	}
	// Opting in must not move a provider between tiers, in either direction.
	// The weight orders fact leaves against each other and nothing else.
	if pricey.class != costFacts {
		t.Errorf("a CostReporter provider = class %d, want costFacts (%d)", pricey.class, costFacts)
	}
	if !costLess(staticTiers{}.leafCost(leafOf(t, bodyLeaf("bb"))), pricey) {
		t.Error("a body leaf is no longer cheaper than a self-declared expensive fact leaf")
	}
}

func TestStaticTiersLeavesNonReportingProvidersAlone(t *testing.T) {
	// The control for the case above: a provider that does not implement
	// CostReporter must weigh one unit, so "optional" means optional rather
	// than "required, defaulting to whatever the last reporter said".
	if got := (staticTiers{}).leafCost(leafOf(t, factLeaf("n", loadingFacts("plain")))); got.weight != 1 {
		t.Fatalf("a plain FactProvider weighs %d, want 1", got.weight)
	}
	mixed := staticTiers{}.leafCost(leafOf(t, factLeaf("m", loadingFacts("plain-2"), pricyProvider{key: "pricy-2"})))
	if mixed.weight != 51 {
		t.Fatalf("a plain provider beside a reporting one weighs %d, want 51", mixed.weight)
	}
}

func TestCostCompareIsAProperOrdering(t *testing.T) {
	// slices.SortStableFunc's contract is a comparison that is antisymmetric and
	// reports ties as 0. Pinned directly, because the sort does not enforce it:
	// a costCompare that answered 1 for a tie — plausible, since costLess-style
	// helpers often return "not less" as "greater" — violates the contract while
	// leaving every ordering test above green, measured. Go's stable sort only
	// ever swaps on a strictly negative answer, so today it happens to survive a
	// comparator that never returns 0. That is an implementation detail of the
	// sort, not a promise, and it is the whole basis of the stability the plan
	// depends on.
	costs := []cost{
		{class: costPure},
		{class: costPure, weight: 3},
		{class: costBody, weight: 1},
		{class: costFacts, weight: 1},
		{class: costFacts, weight: 50},
	}

	for _, left := range costs {
		if got := costCompare(left, left); got != 0 {
			t.Errorf("costCompare(%+v, itself) = %d, want 0", left, got)
		}
		for _, right := range costs {
			forward, backward := costCompare(left, right), costCompare(right, left)
			if forward != -backward {
				t.Errorf("costCompare(%+v, %+v) = %d but the reverse = %d; the comparison is not antisymmetric",
					left, right, forward, backward)
			}
			if (forward < 0) != costLess(left, right) {
				t.Errorf("costCompare(%+v, %+v) = %d disagrees with costLess = %t",
					left, right, forward, costLess(left, right))
			}
		}
	}
}

// ---------------------------------------------------------------------------
// Effectiveness: the calls the pass removes
// ---------------------------------------------------------------------------

func TestSinkFactLoadersSkipsProviderBehindCheapDeny(t *testing.T) {
	// The test the whole plan exists for. Everything in the differential
	// harness is green against a pass that does nothing; this is not.
	//
	// The baseline is in here rather than beside it, because on its own the
	// zero proves nothing — a provider that was never wired up also loads zero
	// times. Only the same tree with optimization off, loading exactly once,
	// makes the zero attributable to the pass. Split across two test functions
	// the assertion could be run by -run without its control, which is the
	// wrong failure mode for a pair whose whole point is that one half is what
	// gives the other meaning.
	optimized := loadingFacts("sink-effectiveness")
	if status := runPolicyStatus(t, All(factLeaf("expensive", optimized), denyLeaf("cheapdeny"))); status != http.StatusForbidden {
		t.Fatalf("optimized status = %d, want %d", status, http.StatusForbidden)
	}
	if optimized.loads != 0 {
		t.Fatalf("fact provider called %d times, want 0 (it should have been sunk behind the cheap deny)", optimized.loads)
	}

	baseline := loadingFacts("sink-baseline")
	if status := runPolicyStatus(t, All(factLeaf("expensive2", baseline), denyLeaf("cheapdeny2")), WithPipeline()); status != http.StatusForbidden {
		t.Fatalf("baseline status = %d, want %d", status, http.StatusForbidden)
	}
	if baseline.loads != 1 {
		t.Fatalf("baseline fact provider called %d times, want 1 (this is what proves the pass is what saves the call)", baseline.loads)
	}
}

func TestSinkFactLoadersSkipsProviderBehindCheapAllowUnderAny(t *testing.T) {
	// Ascending order is correct for Any too, and for the mirror-image reason:
	// a disjunction stops at the first true, so the cheap allow in front is
	// what removes the load. A pass that special-cased All would leave this one
	// loading. Paired with its own baseline for the same reason as above.
	optimized := loadingFacts("sink-any-optimized")
	if status := runPolicyStatus(t, Any(factLeaf("anyexpensive", optimized), allowLeaf("cheapallow"))); status != http.StatusNoContent {
		t.Fatalf("optimized status = %d, want %d", status, http.StatusNoContent)
	}
	if calls := optimized.loads; calls != 0 {
		t.Fatalf("fact provider called %d times, want 0 (it should have been sunk behind the cheap allow)", calls)
	}

	baseline := loadingFacts("sink-any-baseline")
	if status := runPolicyStatus(t, Any(factLeaf("anyexpensive2", baseline), allowLeaf("cheapallow2")), WithPipeline()); status != http.StatusNoContent {
		t.Fatalf("baseline status = %d, want %d", status, http.StatusNoContent)
	}
	if calls := baseline.loads; calls != 1 {
		t.Fatalf("baseline fact provider called %d times, want 1", calls)
	}
}

// ---------------------------------------------------------------------------
// What the pass does to the tree
// ---------------------------------------------------------------------------

func TestSinkSortsAscendingInBothKinds(t *testing.T) {
	provider := loadingFacts("ascending")
	cases := []struct {
		name string
		node Node
	}{
		{"conjunction", All(factLeaf("facts", provider), bodyLeaf("body"), allowLeaf("pure"))},
		{"disjunction", Any(factLeaf("facts", provider), bodyLeaf("body"), allowLeaf("pure"))},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			got := queryOrder(Optimize(testCase.node, []string{"sink-facts"}))
			want := []string{"data.pure.allow", "data.body.allow", "data.facts.allow"}
			if !slices.Equal(got, want) {
				t.Fatalf("order = %v, want %v (cheapest first)", got, want)
			}
		})
	}
}

func TestSinkIsStableForEqualCosts(t *testing.T) {
	// This test can only fail if the input satisfies two conditions at once:
	// more than twelve children, and costs that are not all equal. Both are
	// deliberate, and both were measured against slices.SortFunc standing in for
	// an unstable sort.
	//
	// More than twelve, because sort.Slice and slices.SortFunc are unstable in
	// principle but dispatch to insertion sort — which is stable — for short
	// inputs. pdqsort's cutoff is twelve, so declaration order survives up to
	// twelve children and is scrambled from thirteen. A deterministic threshold,
	// not a probability: a short test does not flake, it simply always passes.
	//
	// Not all equal, because when every comparison returns 0 pdqsort's
	// already-sorted detection short-circuits unconditionally and the input is
	// never permuted — at any size. All-equal input is inert whether it is 3
	// children or 200, so scaling an all-equal test up past twelve fixes
	// nothing. The costs below alternate pure and facts to give the sort real
	// work to do.
	//
	// Both conditions, or the test proves nothing. This is not hypothetical: the
	// version in the plan was three equal-cost leaves, and it was run verbatim
	// under an unstable sort and passed.
	//
	// Twenty-four rather than thirteen for margin, because the cutoff is a Go
	// runtime internal: if a future toolchain raises it past the child count,
	// this test goes quietly green against an unstable sort.
	const children = 24

	var declared []Node
	var wantPure, wantFacts []string
	for index := 0; index < children; index++ {
		name := fmt.Sprintf("s%02d", index)
		if index%2 == 0 {
			declared = append(declared, denyLeaf(name))
			wantPure = append(wantPure, "data."+name+".allow")
			continue
		}
		declared = append(declared, factLeaf(name, loadingFacts(name)))
		wantFacts = append(wantFacts, "data."+name+".allow")
	}

	got := queryOrder(Optimize(All(declared...), []string{"sink-facts"}))
	want := append(append([]string{}, wantPure...), wantFacts...)
	if !slices.Equal(got, want) {
		t.Fatalf("order =\n%v\nwant\n%v\n(equal-cost leaves must keep declaration order)", got, want)
	}
}

func TestSinkUsesTheMinimumChildCostForInteriorNodes(t *testing.T) {
	// An interior node is priced at its cheapest child, because after the
	// bottom-up sort that child is in front and is the only one lazy evaluation
	// is certain to pay for.
	//
	// The sibling sits strictly between the subtree's minimum and its maximum,
	// which is what makes min and max distinguishable here: priced at its
	// minimum the subtree is pure and moves in front of the body leaf; priced
	// at its maximum it is facts and stays behind it.
	//
	// The subtree is declared dearest-first so that its own sort has to happen
	// too. A subtree already in cheap-first order would let a pass that priced
	// interior nodes correctly but never recursed pass this.
	provider := loadingFacts("interior")
	node := All(bodyLeaf("sibling"), Any(factLeaf("dearinner", provider), allowLeaf("cheapinner")))

	got := queryOrder(Optimize(node, []string{"sink-facts"}))
	want := []string{"data.cheapinner.allow", "data.dearinner.allow", "data.sibling.allow"}
	if !slices.Equal(got, want) {
		t.Fatalf("order = %v, want %v (the subtree costs its minimum, so it sorts ahead of the body leaf)", got, want)
	}
}

func TestSinkRecursesIntoNestedNodes(t *testing.T) {
	// Two levels below the root, and under both kinds, so a pass that sorted
	// only the node it was handed — or only one composite kind — is caught.
	provider := loadingFacts("nested")
	node := All(
		Any(
			factLeaf("deepfacts", provider),
			All(bodyLeaf("deepbody"), allowLeaf("deeppure")),
		),
	)

	got := queryOrder(Optimize(node, []string{"sink-facts"}))
	want := []string{"data.deeppure.allow", "data.deepbody.allow", "data.deepfacts.allow"}
	if !slices.Equal(got, want) {
		t.Fatalf("order = %v, want %v", got, want)
	}
}

func TestSinkLeavesChildlessCompositesForCompileToReject(t *testing.T) {
	// Sorting permutes, so it cannot empty a composite — but it also must not
	// swallow one it was handed. A childless conjunction is the one rewrite
	// outcome that turns into an unconditional allow rather than a wrong
	// answer, and only compileChildren stands between it and a 200.
	node := AllNode{Children: []Node{allowLeaf("a"), AnyNode{}}}
	got := Optimize(node, []string{"sink-facts"})

	if empty := emptyCompositeIn(got); empty == nil {
		t.Fatalf("sink-facts swallowed the childless composite:\n%s", Explain(got))
	}
	if _, err := Compile(got); err == nil {
		t.Fatalf("Compile accepted a tree with a childless composite:\n%s", Explain(got))
	}
}

// ---------------------------------------------------------------------------
// Pass ordering
// ---------------------------------------------------------------------------

func TestFlattenAndSinkDoNotCommuteWithinOneSweep(t *testing.T) {
	// TestStructuralPassesCommuteToTheSameFixpoint records that flatten,
	// collapse and dedupe can be run in any order because each only exposes
	// work for the others. sink-facts is the pass that ends that: its output is
	// not a normal form, and a nested composite is a wall it cannot sort
	// across.
	//
	// All(All(facts, pureA), pureB): flattened first, all three leaves are
	// siblings and pureB gets in front of the fact loader. Sunk first, the
	// nesting still separates them, and flattening afterwards leaves the fact
	// load ahead of a pure check that could have short-circuited it.
	provider := loadingFacts("ordering")
	node := All(All(factLeaf("ordfacts", provider), allowLeaf("puream")), allowLeaf("purebee"))

	flattenFirst := queryOrder(sinkFactLoaders(flatten(node)))
	sinkFirst := queryOrder(flatten(sinkFactLoaders(node)))

	wantFlattenFirst := []string{"data.puream.allow", "data.purebee.allow", "data.ordfacts.allow"}
	if !slices.Equal(flattenFirst, wantFlattenFirst) {
		t.Fatalf("flatten then sink = %v, want %v", flattenFirst, wantFlattenFirst)
	}
	wantSinkFirst := []string{"data.puream.allow", "data.ordfacts.allow", "data.purebee.allow"}
	if !slices.Equal(sinkFirst, wantSinkFirst) {
		t.Fatalf("sink then flatten = %v, want %v", sinkFirst, wantSinkFirst)
	}
	if slices.Equal(flattenFirst, sinkFirst) {
		t.Fatalf("the two orders agree (%v); this test no longer pins an ordering dependence", flattenFirst)
	}

	// Optimize's fixpoint loop rescues the wrong order — it re-runs sink after
	// flatten on the next sweep — but it pays a sweep for it. That is the
	// concrete cost of the ordering in defaultPipeline, so it is measured
	// rather than asserted in prose. The counter is appended, so it runs once
	// per sweep without changing what converges.
	sweeps := func(pipeline ...string) int {
		count := 0
		name := fmt.Sprintf("test-sweep-%s", strings.Join(pipeline, "-"))
		registerTestPass(t, name, func(n Node) Node {
			count++
			return n
		})
		Optimize(node, append(slices.Clone(pipeline), name))
		return count
	}

	rightOrder := sweeps("flatten", "sink-facts")
	wrongOrder := sweeps("sink-facts", "flatten")
	t.Logf("sweeps to converge: flatten-then-sink %d, sink-then-flatten %d", rightOrder, wrongOrder)
	if rightOrder >= wrongOrder {
		t.Fatalf("flatten-then-sink took %d sweeps and sink-then-flatten took %d; the default pipeline's order buys nothing",
			rightOrder, wrongOrder)
	}
}

// ---------------------------------------------------------------------------
// Invariants over the generated corpus
// ---------------------------------------------------------------------------

func TestSinkPreservesTheLeafMultisetOverGeneratedTrees(t *testing.T) {
	// A reordering pass may permute; it may not add, drop or substitute. Run
	// over the whole generated corpus rather than one hand-built tree, because
	// the failure mode is a slice-handling slip that only some shapes reach.
	for seed := int64(1); seed <= equivalenceSeeds; seed++ {
		node := genTree(rand.New(rand.NewSource(seed)), equivalenceDepth).astNode()
		sunk := sinkFactLoaders(node)

		before, after := leafMultiset(node), leafMultiset(sunk)
		if !slices.Equal(before, after) {
			t.Fatalf("seed %d: leaf multiset changed\n--- before ---\n%s\n--- after ---\n%s",
				seed, Explain(node), Explain(sunk))
		}
		if again := sinkFactLoaders(sunk); Explain(again) != Explain(sunk) {
			t.Fatalf("seed %d: sink-facts is not idempotent\n--- once ---\n%s\n--- twice ---\n%s",
				seed, Explain(sunk), Explain(again))
		}
	}
}

func TestSinkRewritesAFairShareOfGeneratedTrees(t *testing.T) {
	// The floor that says the pass does something on real input. Measured, it
	// reorders 213 of 300 generated trees on its own.
	const wantAtLeast = equivalenceSeeds / 4

	rewritten := 0
	for seed := int64(1); seed <= equivalenceSeeds; seed++ {
		node := genTree(rand.New(rand.NewSource(seed)), equivalenceDepth).astNode()
		if Explain(sinkFactLoaders(node)) != Explain(node) {
			rewritten++
		}
	}

	t.Logf("sink-facts reordered %d of %d generated trees", rewritten, equivalenceSeeds)
	if rewritten < wantAtLeast {
		t.Errorf("sink-facts reordered %d of %d trees, want at least %d", rewritten, equivalenceSeeds, wantAtLeast)
	}
}
