package auth

import (
	"fmt"
	"math/rand"
	"slices"
	"testing"
)

// The optimizer's differential harness, driven by the fuzzer instead of by a
// fixed seed range. It reuses the reference oracle and the comparison in
// policy_equivalence_test.go wholesale; the only thing this file adds is a
// second way to reach a generated tree.
//
// Not run in CI as a fuzzing campaign — `go test` runs a fuzz target over its
// seed corpus and stops, which is what keeps the committed crashers in
// testdata/fuzz working as ordinary regression tests. Soaking it is a local
// step when changing a pass:
//
//	go test ./auth/ -run FuzzOptimizerEquivalence -fuzz FuzzOptimizerEquivalence -fuzztime 60s
//
// testdata/fuzz holds the two crashers that soak has produced so far, each
// found under a mutated key and each replaying as a two-leaf tree over the
// shared module:
//
//	01011001  Any(deny, allow),     found in 7.5s against a dedupe keyed on the
//	                                source alone: the merge keeps the deny and
//	                                answers 403 where the policy says 204.
//	11021011  All(factError, deny), found in 4.6s against a Leaf.key() with the
//	                                facts component dropped: the merge keeps the
//	                                error and answers 500 where it says 403.
//
// Eight bytes each, and between them the smallest statement of what this file
// is for.
//
// What it can and cannot show is worth stating, because the seed range next
// door has the same limit: like every test that compares an optimized plan with
// an unoptimized reference, this cannot tell a working optimizer from one that
// returns its input. A pass that does nothing preserves semantics perfectly.
// TestDefaultPipelineRewritesGeneratedTrees is the other half of that pair.

// tape turns the fuzzer's byte string into the generator's decisions, one byte
// per decision.
//
// That is why the target takes a byte string rather than the int64 seed the
// plan sketched. A seed has no structure to mutate: flipping one of its bits
// redraws the entire tree, so coverage-guided mutation has no gradient to
// follow and the fuzzer is reduced to sampling the same generator the fixed
// seed range already samples exhaustively. Against a tape, flipping a byte
// moves one decision and leaves the rest of the tree standing.
type tape struct {
	bytes []byte
	at    int
}

// intn consumes one byte. A tape that has run out answers 0 forever, which is
// what makes every tape finite: genSubtree reads 0 as "stop here and emit a
// leaf", so running off the end closes the tree off instead of looping. The
// modulo is biased towards low values for an n that does not divide 256; the
// bias costs nothing here, because the fuzzer is free to reach any value of any
// draw by mutating its byte.
func (source *tape) intn(n int) int {
	if source.at >= len(source.bytes) {
		return 0
	}
	value := int(source.bytes[source.at]) % n
	source.at++
	return value
}

// recordingDraws writes down every decision another source makes, so that a
// tape can be produced from math/rand rather than assembled by hand. Recording
// the *result* rather than the raw byte is what makes the replay exact: every n
// the generator asks for is far below 256, so value % n is value again.
type recordingDraws struct {
	inner drawSource
	tape  []byte
}

func (source *recordingDraws) intn(n int) int {
	value := source.inner.intn(n)
	source.tape = append(source.tape, byte(value))
	return value
}

// fuzzGenConfig is the widened generator. Every source variant, so leaves that
// share a module while differing in query, facts or body limit are reachable —
// see sourceVariant for the measured hole that shape closes. The depth is the
// seed range's, so a tape can express anything a seed can.
var fuzzGenConfig = genConfig{depth: equivalenceDepth, sourceVariants: int(sourceVariantCount)}

// fuzzSeedTapes is how many math/rand-drawn tapes seed the corpus. Small on
// purpose: every entry is replayed in full by an ordinary `go test` run, and one
// entry costs a tree build, an Optimize — which renders the whole tree once per
// sweep — and two Rego compilations of every surviving leaf.
const fuzzSeedTapes = 20

// fuzzSearchSeeds bounds the search below. The two shapes it looks for are rare
// among random draws — the narrower one needs two particular behaviors landing
// under one parent, both drawn from the shared module — so the bound is far
// above where they are actually found, and a search that runs out is a
// generator that lost the shape rather than a search that was unlucky.
const fuzzSearchSeeds = 1000

// recordTape draws a tree from a fixed seed and returns it beside the tape that
// reproduces it.
func recordTape(seed int64) (refTree, []byte) {
	source := &recordingDraws{inner: randDraws{rng: rand.New(rand.NewSource(seed))}}
	tree := genTreeFrom(source, fuzzGenConfig)
	return tree, source.tape
}

// searchTape looks for the lowest seed whose tree puts a pair of leaves
// matching wanted under one parent, and returns the tape for it.
//
// Searched rather than written out as bytes. A hand-written tape is a magic
// number that says nothing about why those bytes were chosen and quietly stops
// meaning it the moment the generator draws in a different order; a search says
// what the entry is for and fails loudly when the shape becomes unreachable.
func searchTape(wanted func(one, other Leaf) bool) (script []byte, seed int64, found bool) {
	for candidate := int64(1); candidate <= fuzzSearchSeeds; candidate++ {
		tree, recorded := recordTape(candidate)
		for _, pair := range siblingLeafPairs(tree.astNode()) {
			if wanted(pair[0], pair[1]) {
				return recorded, candidate, true
			}
		}
	}
	return nil, 0, false
}

// fuzzWantedShapes is one entry per way two sibling leaves can differ, and the
// corpus carries a tape for each. Enumerated rather than sampled: a differential
// harness is blind to an unsound merge it never generates the operands for, so
// "which differences can the generator produce" is the question that decides
// what this target can catch, and it deserves a list rather than a hope.
var fuzzWantedShapes = []struct {
	name  string
	match func(one, other Leaf) bool
}{
	{"two leaves sharing a source and differing", sharesSourceAndDiffers},
	{"two leaves differing in nothing but their query", differsOnlyInQuery},
	{"two leaves differing in nothing but their facts", differsOnlyInFacts},
	{"two leaves differing in nothing but their body limit", differsOnlyInBodyLimit},
}

// fuzzSeedCorpus is the corpus the fuzz target starts from, built once so that
// the target and the test that measures it cannot be looking at different sets
// of tapes.
//
// It fails rather than returning a smaller corpus, because the searched entries
// are the entire reason this file is not a re-run of the fixed seed range.
// Silently seeding without them would leave a target that passes, soaks, and
// proves what the 300 seeds already proved.
func fuzzSeedCorpus(fail func(format string, args ...any)) [][]byte {
	corpus := [][]byte{
		// The empty tape, which no recording produces: every draw runs off its
		// end. It is the smallest input the fuzzer has to grow from, and it pins
		// that an exhausted tape still yields a tree the harness can run rather
		// than a panic or an empty composite Compile refuses.
		{},
	}
	for seed := int64(1); seed <= fuzzSeedTapes; seed++ {
		_, script := recordTape(seed)
		corpus = append(corpus, script)
	}
	for _, wanted := range fuzzWantedShapes {
		script, _, found := searchTape(wanted.match)
		if !found {
			fail("no tape in %d seeds puts %s under one parent; the generator has lost the shape this target exists to reach",
				fuzzSearchSeeds, wanted.name)
			continue
		}
		corpus = append(corpus, script)
	}
	return corpus
}

func FuzzOptimizerEquivalence(f *testing.F) {
	for _, script := range fuzzSeedCorpus(f.Fatalf) {
		f.Add(script)
	}

	f.Fuzz(func(t *testing.T, script []byte) {
		tree := genTreeFrom(&tape{bytes: script}, fuzzGenConfig)
		// The tree's signature, not the tape: it is what a failure has to be
		// read against, and two tapes differing only past the point the
		// generator stopped reading describe the same tree.
		assertMatchesReference(t, optimizedMode, tree.signature(), tree)
	})
}

func TestFuzzSeedTapesReplayTheirTrees(t *testing.T) {
	// The seed corpus is only as good as this: a tape has to make the same
	// decisions on replay that math/rand made when it was recorded. If it did
	// not, the corpus would be a set of trees nobody chose, and the claim that
	// fuzzing starts where the fixed seed range starts would be empty.
	for seed := int64(1); seed <= fuzzSeedTapes; seed++ {
		recorded, script := recordTape(seed)
		replayed := genTreeFrom(&tape{bytes: script}, fuzzGenConfig)
		if replayed.signature() != recorded.signature() {
			t.Errorf("seed %d: tape %x replays as %s, want %s", seed, script, replayed.signature(), recorded.signature())
		}
	}
}

// siblingLeafPairs returns every pair of leaves the tree puts under one parent.
// Siblings are the unit that matters: dedupe compares a node's children with
// each other and nothing else, so two leaves that never meet under one parent
// are two leaves it will never be asked to merge.
func siblingLeafPairs(n Node) [][2]Leaf {
	var children []Node
	switch node := n.(type) {
	case AllNode:
		children = node.Children
	case AnyNode:
		children = node.Children
	case BecauseNode:
		return siblingLeafPairs(node.Child)
	default:
		return nil
	}

	pairs := [][2]Leaf{}
	leaves := []Leaf{}
	for _, child := range children {
		leaf, ok := child.(Leaf)
		if !ok {
			pairs = append(pairs, siblingLeafPairs(child)...)
			continue
		}
		for _, earlier := range leaves {
			pairs = append(pairs, [2]Leaf{earlier, leaf})
		}
		leaves = append(leaves, leaf)
	}
	return pairs
}

// A leaf can differ from another on four axes, one per component of
// Leaf.key(): source, query, facts, body limit. The predicates below name the
// coarse shape and then a pair differing in exactly one axis, which is what a
// dedupe keyed on any proper subset of the key would merge.
//
// Three of the four are pinned, each measured by dropping that one component
// from key() and watching this target go red while all 300 fixed seeds stayed
// green:
//
//	query:  seed#6 fails                     (behaviorAllow vs behaviorRegoError)
//	facts:  crasher 35f2b0c92e371f36 fails   (behaviorDeny  vs behaviorFactError)
//	body:   seed#24 fails                    (behaviorAllow vs behaviorBodyOverLimit)
//
// The source component is deliberately not pinned that way, and the reason is
// worth having written down rather than rediscovered. Dropping it alone from
// key() survives a 90s soak — 40823 execs, green — because no generated pair
// can differ in source alone: every leaf's query names its own package, so a
// source difference always drags a query difference along with it.
//
// Reaching it would take two modules sharing a package name, differing in text,
// and *disagreeing about the rule being queried* — anything less and the merge
// is invisible to the oracle, which compares verdicts. That last requirement is
// the disqualifying one: the reference would then have to know which module a
// leaf came from, and "the oracle is a function of behavior alone" is the
// property this whole harness rests on. Trading it for one more axis is a bad
// bargain.
//
// What covers the source axis instead is the direction that actually threatens
// an authorization plan. Dedupe's failure mode is over-merging, and the coarse
// mutation for that — keying on the source *alone* — is caught here in 0.12s.
// Under-discriminating on source is left to the hand-written cases in
// TestDedupeKeepsLeavesThatAreNotProvablyIdentical ("different sources", "same
// source name, different text") and TestDedupeNeverMergesLeavesWithoutStableIdentity,
// which is the right home for it: those are claims about identity, not about
// what a tree decides.
//
// They compare the leaf's fields directly rather than asking key() whether two
// leaves collide, and that is load-bearing rather than laborious. A predicate
// written in terms of key() cannot judge key(): with the facts component
// dropped from it — the mutation that put this axis here — the facts-only pair
// stops being visible to its own search, and the target then fails by claiming
// the generator lost a shape instead of by catching the unsound merge. The cost
// is a second encoding of what a fact list means, and it is the same trade
// refTruth makes against truth in policy_equivalence_test.go: an oracle that
// imports its subject's definition of the answer can only ever confirm it.
//
// The exposure that buys is drift in the other direction. If Leaf gains a fifth
// component, key() will compare it and these will not, so "differs only in X"
// becomes "differs in X and possibly the new field". The new axis would then be
// unpinned rather than wrongly pinned, and TestLeafKeyIsInjective is where a
// fifth component gets noticed.

// sharesSource reports whether two leaves come from the same module. An empty
// source key means "no stable identity", which is never equal to anything,
// including another empty one. No generated leaf has one — they are all Inline
// — but treating two of them as a match would be the opposite of what this asks.
func sharesSource(one, other Leaf) bool {
	key := one.Source.sourceKey()
	return key != "" && key == other.Source.sourceKey()
}

// sameFacts reports whether two leaves declare the same fact loads: the same
// namespaces bound to the same cache keys, in any order, because forPolicy
// builds a map from them and order is not meaning.
func sameFacts(one, other Leaf) bool {
	return slices.Equal(factIdentities(one), factIdentities(other))
}

func factIdentities(leaf Leaf) []string {
	identities := make([]string, 0, len(leaf.Facts))
	for _, configured := range leaf.Facts {
		identities = append(identities, fmt.Sprintf("%q=%q", configured.namespace, configured.provider.CacheKey()))
	}
	slices.Sort(identities)
	return identities
}

// sharesSourceAndDiffers is the coarsest shape the fixed seed range cannot
// reach: two leaves that agree on their module and disagree about what to ask
// it. It is exactly what a dedupe keyed on Leaf.Source.sourceKey() alone would
// merge, and merging it deletes an authorization check.
func sharesSourceAndDiffers(one, other Leaf) bool {
	return sharesSource(one, other) &&
		(one.Query != other.Query || !sameFacts(one, other) || one.MaxBody != other.MaxBody)
}

func differsOnlyInQuery(one, other Leaf) bool {
	return sharesSource(one, other) &&
		one.Query != other.Query && sameFacts(one, other) && one.MaxBody == other.MaxBody
}

// differsOnlyInFacts is the axis that went missing when this file was first
// written, and the reason the axes are now enumerated rather than hoped for:
// behaviorFactError was the only scripted leaf carrying a provider and also the
// only one naming its rule, so a facts difference could not arise without a
// query difference beside it. Measured, a Leaf.key() with the facts component
// dropped passed this target and all 300 fixed seeds; only hand-written unit
// tests caught it. behaviorDeny now names the same rule with nothing to load,
// which closes it.
func differsOnlyInFacts(one, other Leaf) bool {
	return sharesSource(one, other) &&
		one.Query == other.Query && !sameFacts(one, other) && one.MaxBody == other.MaxBody
}

// differsOnlyInBodyLimit reaches one leaf that allows beside one that must
// answer 400, identical in every other respect. A dedupe that compared
// everything but MaxBody would answer 204 for both.
func differsOnlyInBodyLimit(one, other Leaf) bool {
	return sharesSource(one, other) &&
		one.Query == other.Query && sameFacts(one, other) && one.MaxBody != other.MaxBody
}

func TestFuzzSeedCorpusReachesWhatTheFixedSeedRangeCannot(t *testing.T) {
	// The reason this file is not a second run of the 300 seeds. Measured, not
	// assumed: mutating dedupeChildren to key on the source alone — a merge that
	// is genuinely unsound — leaves every one of those 300 seeds green, because
	// there a source implies the query, the facts and the body limit. Both
	// shapes below break that implication, and the corpus has to contain them
	// before the target is worth soaking.
	//
	// Counted over the corpus the target actually seeds itself with, so an entry
	// that stopped carrying its shape — or a tape that replayed as some other
	// tree — is caught here rather than assumed away.
	//
	// What this does *not* establish is that the seed corpus catches an unsound
	// merge of each shape, and the distinction is not academic: a pair has to sit
	// somewhere the merge changes the fold before any verdict moves. All(deny,
	// factError) folds to false either way, because false annihilates a
	// conjunction whether or not the error beside it survived — so the facts-only
	// pair was in the corpus, was merged by a key with the facts dropped, and
	// changed nothing. Only Any(deny, factError), or the same pair the other way
	// round under All, is observable. Placing a pair is the fuzzer's job; having
	// the operands at all is this test's.
	corpus := fuzzSeedCorpus(t.Fatalf)
	found := make([]int, len(fuzzWantedShapes))
	for _, script := range corpus {
		tree := genTreeFrom(&tape{bytes: script}, fuzzGenConfig)
		for _, pair := range siblingLeafPairs(tree.astNode()) {
			for index, wanted := range fuzzWantedShapes {
				if wanted.match(pair[0], pair[1]) {
					found[index]++
				}
			}
		}
	}

	for index, wanted := range fuzzWantedShapes {
		t.Logf("%d sibling leaf pairs are %s, over %d seed tapes", found[index], wanted.name, len(corpus))
		if found[index] == 0 {
			t.Errorf("no seed tape puts %s under a single parent", wanted.name)
		}
	}
}

func TestFuzzSearchTapeReportsWhenNothingMatches(t *testing.T) {
	// searchTape is what puts the two sharp entries in the corpus, and a version
	// that reported success for a predicate nothing satisfies would seed an
	// arbitrary tape while fuzzSeedCorpus's failure path went untested. Nothing
	// else in this file ever asks it a question with no answer.
	if script, seed, found := searchTape(func(Leaf, Leaf) bool { return false }); found {
		t.Fatalf("searchTape found seed %d (tape %x) for a predicate that matches nothing", seed, script)
	}
}
