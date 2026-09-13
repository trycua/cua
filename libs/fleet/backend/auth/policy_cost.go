package auth

import "slices"

// This file gives the optimizer a reason to prefer one child order over
// another, and the pass that acts on it.
//
// Reordering children is legal because All is min and Any is max over
// truthFalse < truthError < truthTrue (see policy_plan.go), and min and max are
// commutative. That says every order computes the same answer; it says nothing
// about what each order costs to compute. The cost model below is what turns
// "any order will do" into "this order is the cheap one", and sinkFactLoaders
// is what applies it: cheap request-attribute checks move in front of leaves
// that would buffer a body or call a database, so a cheap deny can
// short-circuit an expensive call away entirely.

// costClass is the coarse tier of a leaf's evaluation cost, ordered cheapest
// first. The tier dominates the comparison, so a class boundary is a promise:
// no weight, however large or however nonsensical a provider reports, can float
// a database call in front of a pure check.
type costClass uint8

const (
	costPure  costClass = iota // method, path, route, params, user, flags
	costBody                   // buffers the raw request body
	costFacts                  // calls a FactProvider: a database or an API
)

// Compile-time assertions on the ordering, in the same spirit as the lattice
// assertions in policy_plan.go. costLess compares classes with <, so permuting
// the block above does not produce a compile error anywhere — it silently
// inverts what the pass considers cheap. These fail to compile instead.
//
// The subtraction is what overflows; the uint conversion is belt-and-braces so
// the assertions keep working if costClass ever becomes a signed type, where a
// negative difference would otherwise be legal. Written the same way as
// policy_plan.go's for the same reason, so the two blocks stay recognisably one
// idiom.
const (
	_ = uint(costBody - costPure)
	_ = uint(costFacts - costBody)

	// costPure must be the zero value, so that a cost{} — the answer for a node
	// kind this file does not recognise, and for a childless composite — reads
	// as "free" rather than as an accidental tier.
	_ = uint(0 - costPure)
)

// cost is an estimate of what evaluating a node costs. class is the tier;
// weight only ever breaks ties within one tier.
type cost struct {
	class  costClass
	weight int
}

func costLess(left, right cost) bool {
	if left.class != right.class {
		return left.class < right.class
	}
	return left.weight < right.weight
}

// costCompare orders two costs for a stable sort: negative when left is
// cheaper, zero when neither is cheaper than the other. Returning 0 for ties
// rather than 1 is what lets the stable sort see them as ties at all.
func costCompare(left, right cost) int {
	switch {
	case costLess(left, right):
		return -1
	case costLess(right, left):
		return 1
	default:
		return 0
	}
}

// costModel estimates a leaf's cost. It is an interface so that adaptive,
// statistics-driven planning can arrive later as a second implementation
// rather than as a rewrite of sinkFactLoaders.
//
// The seam is narrower than that sentence suggests, and the limit is worth
// knowing before anyone relies on it: a model prices leaves. How an interior
// node is priced from its children is sinkChildren's rule — the minimum — not
// the model's. An adaptive model is exactly the one that would want a say
// there, pricing a subtree from observed short-circuit rates rather than from
// its cheapest child, and it would have to change sinkChildren to get it.
//
// It is unexported, with an unexported method, because nothing outside this
// package can supply one: the design doc reserves a WithCostModel plan option
// for the adaptive case, and until that option exists an exported interface
// would be one nobody could pass anywhere. The shape is what matters and the
// shape is preserved; exporting it later is a rename.
type costModel interface {
	leafCost(Leaf) cost
}

// CostReporter is an optional interface a FactProvider may implement to declare
// that it is dearer than the default. Implementing it is optional in the
// strongest sense: staticTiers type-asserts it, so a provider that does not
// implement it is weighed as one unit and is otherwise unaffected. That is why
// FactProvider itself did not have to change, and why providers living outside
// this package — the reason this one interface is exported — can opt in one at
// a time.
//
// The number is a relative weight, not a latency or a currency. It orders
// fact-loading leaves against each other and nothing else: costFacts is still
// costFacts, so a provider reporting 0 does not become as cheap as a path
// check, and one reporting a huge number cannot push a body-reading sibling
// below a pure one. A negative number is accepted and does the same thing in
// the other direction — it can only move a leaf earlier among fact leaves — so
// there is nothing to guard, but there is also no reason to return one.
//
// FactCost is asked while the plan is built, never while a request runs: it is
// called from Optimize during route construction, and Optimize repeats its
// pipeline to a fixpoint, so one provider may be asked several times for one
// route. Return a constant. A value derived from live statistics — a rolling
// p99, a queue depth — would be read at startup and then frozen into the plan
// anyway, and if it drifted between calls within one Optimize it would keep
// changing the tree and burn sweeps against the fixpoint cap to no purpose.
// Adaptive planning is a costModel's job, not a provider's.
//
// Weights add up across the distinct cache keys on one leaf. A leaf carrying
// one plain provider and one reporting 50 weighs 51, so a provider is declaring
// its own share of a total rather than the whole leaf's cost.
type CostReporter interface {
	FactCost() int
}

// staticTiers derives cost purely from a leaf's declared configuration. It
// reads nothing but the Leaf, so it is deterministic and needs no request.
type staticTiers struct{}

// leafCost tiers a leaf by the dearest thing it needs.
//
// Facts are checked first, so a leaf carrying both fact providers and a body
// limit classifies as costFacts: it will do both, and the dearer of the two is
// what the tier is for. Ordering the checks the other way would hide a database
// call behind a body limit.
//
// Within costFacts the weight is the number of *distinct* cache keys, not the
// number of providers. Fact results are cached per request across the whole
// tree (see requestPolicyInput.resolveFacts), so a second consumer of a key
// already loaded costs nothing, and counting it would make a leaf that reuses
// one provider look dearer than one that loads two.
//
// The first provider listed for a cache key is the one charged; later ones are
// skipped before their CostReporter is consulted. That only becomes visible if
// two *different* provider values share a CacheKey and disagree about their
// cost, in which case the weight depends on their order in leaf.Facts. Sharing
// a cache key already asserts they are the same load — resolveFacts will call
// exactly one of them and hand the result to both — so declaring different
// costs for it is a contradiction in the input rather than a case to arbitrate.
// It is written down because it is the one answer this model gives that depends
// on something the author did not know they were choosing.
func (staticTiers) leafCost(leaf Leaf) cost {
	if len(leaf.Facts) > 0 {
		seen := make(map[string]bool, len(leaf.Facts))
		weight := 0
		for _, configured := range leaf.Facts {
			key := configured.provider.CacheKey()
			if seen[key] {
				continue
			}
			seen[key] = true
			if reporter, ok := configured.provider.(CostReporter); ok {
				weight += reporter.FactCost()
				continue
			}
			weight++
		}
		return cost{class: costFacts, weight: weight}
	}
	if leaf.MaxBody > 0 {
		return cost{class: costBody, weight: 1}
	}
	return cost{class: costPure}
}

// defaultCostModel is the model every pipeline uses today. It is a package
// variable rather than a constant expression so that the future WithCostModel
// option has something to displace.
var defaultCostModel costModel = staticTiers{}

// sinkFactLoaders stable-sorts each node's children by ascending cost, so that
// cheap request-attribute checks get the chance to short-circuit expensive fact
// loads away.
//
// Ascending is correct for both node kinds, with no special case: All stops at
// the first false, so a cheap deny in front saves the expensive leaf, and Any
// stops at the first true, so a cheap allow in front does. Either way the
// cheapest child is the one lazy evaluation is guaranteed to pay for.
//
// This is sound only because of the lattice in policy_plan.go. Under the
// earlier abort-on-error semantics the verdict depended on which child failed
// first, and moving a fallible leaf could turn a 500 into a 200.
func sinkFactLoaders(n Node) Node {
	sunk, _ := sinkNode(n, defaultCostModel)
	return sunk
}

// sinkNode returns the rewritten node and what it costs, in one pass. The cost
// travels back up with the node because a parent needs its children's costs to
// sort them, and computing them separately would walk each subtree once per
// comparison.
func sinkNode(n Node, model costModel) (Node, cost) {
	switch node := n.(type) {
	case Leaf:
		// Returned as-is, never rebuilt. Compile rejects a Leaf with a nil
		// Source, and a pass that reassembled one from parts is how that
		// happens; there is nothing here this pass needs to change.
		return node, model.leafCost(node)
	case AllNode:
		children, lowest := sinkChildren(node.Children, model)
		return AllNode{Children: children}, lowest
	case AnyNode:
		children, lowest := sinkChildren(node.Children, model)
		return AnyNode{Children: children}, lowest
	case BecauseNode:
		child, childCost := sinkNode(node.Child, model)
		return BecauseNode{Child: child, Reason: node.Reason}, childCost
	}
	// An unknown node kind is not something to guess a cost for. cost{} is the
	// cheapest value, so it sorts first and is evaluated first — and Compile
	// then refuses the tree by name.
	return n, cost{}
}

// sinkChildren rewrites each child bottom-up, sorts the results by ascending
// cost, and reports the minimum.
//
// The minimum is the right cost for the interior node because the recursion is
// bottom-up: by the time this returns, the cheapest child is the one sitting in
// front, and under lazy evaluation that is the only child the parent is certain
// to pay for. Reporting the maximum would price a subtree by work it may well
// short-circuit past.
//
// Sorting permutes; it cannot empty a composite, so no childless node is
// created here. An input that was already childless is passed through with
// cost{} — malformed input that compileChildren rejects by name, and not
// something to invent a tier for.
func sinkChildren(children []Node, model costModel) ([]Node, cost) {
	type priced struct {
		node Node
		cost cost
	}

	// The minimum is accumulated here rather than read off sorted[0] afterwards,
	// which the ascending sort makes the same value. Two separate claims — "the
	// pass sorts ascending" and "an interior node is priced at its minimum" —
	// are worth being able to break, and test, one at a time.
	sorted := make([]priced, len(children))
	lowest := cost{}
	for index, child := range children {
		node, childCost := sinkNode(child, model)
		sorted[index] = priced{node: node, cost: childCost}
		if index == 0 || costLess(childCost, lowest) {
			lowest = childCost
		}
	}

	// Stable, so declaration order is the tiebreak. That preserves author
	// intent among equally cheap checks, makes the pass idempotent — an
	// unstable sort could permute ties differently on a second run, and
	// Optimize's fixpoint loop runs every pass at least twice — and keeps a
	// plan reproducible across replicas rather than depending on a sort's
	// internal pivot choices.
	slices.SortStableFunc(sorted, func(left, right priced) int {
		return costCompare(left.cost, right.cost)
	})

	nodes := make([]Node, len(sorted))
	for index, child := range sorted {
		nodes[index] = child.node
	}
	return nodes, lowest
}
