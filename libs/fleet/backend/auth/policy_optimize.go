package auth

import (
	"fmt"
	"slices"
)

// This file rewrites policy trees. Every pass here is justified by an algebraic
// property of the evaluator's lattice rather than by inspection: All is min and
// Any is max over truthFalse < truthError < truthTrue (see policy_plan.go), and
// min and max are associative, idempotent and commutative. flatten is
// associativity, collapseUnary is a fold over one element, dedupe is
// idempotence, and sinkFactLoaders — in policy_cost.go, with the cost model it
// sorts by — is commutativity.
//
// None of the four was legal before that lattice existed. Under the earlier
// short-circuit-on-error semantics the verdict depended on which child failed
// first, so merging, dropping or reordering children could change the answer.
// The differential harness in policy_equivalence_test.go is what keeps that
// honest: it runs the whole default pipeline against a total reference
// evaluator over 300 generated trees.

// optimizer rewrites a policy AST. Optimizers are total: a pass that cannot
// safely apply a rewrite returns its input unchanged rather than failing, so a
// buggy or over-cautious pass can never take down startup.
//
// Passes are unexported because a pipeline selects them by name and there is
// no way to register one from outside. An exported pass would be a function
// nothing could ask this package to run.
type optimizer func(Node) Node

// optimizerRegistry maps pass names to implementations. Pipelines are ordered
// name lists because pass order changes the result.
//
// It is written at init and, in this package's tests, by registerTestPass;
// Optimize only reads it. That is safe only while no test runs in parallel, so
// nothing in this package may call t.Parallel.
var optimizerRegistry = map[string]optimizer{
	"flatten":    flatten,
	"collapse":   collapseUnary,
	"dedupe":     dedupe,
	"sink-facts": sinkFactLoaders,
}

// defaultPipeline is ordered deliberately. flatten must precede sink-facts,
// because All(cheap, All(needsDB, cheap)) cannot hoist the inner cheap leaf
// past the DB leaf until the nesting is gone. sink-facts runs last because its
// output is not a normal form.
var defaultPipeline = []string{"flatten", "collapse", "dedupe", "sink-facts"}

// DefaultPipeline is the pass list PolicyMiddleware runs when no pipeline is
// given. It returns a fresh slice: the default decides which rewrites every
// route's authorization plan receives, and an exported []string could be
// rewritten from anywhere with nothing to report it.
func DefaultPipeline() []string { return slices.Clone(defaultPipeline) }

const optimizeFixpointLimit = 8

// Optimize runs the named passes in order, repeating until the tree stops
// changing or the sweep cap is reached. One sweep is not always enough:
// collapse can expose a merge for flatten, so the two only settle when run to a
// fixpoint.
//
// Exhausting the cap does not imply a broken pass. A correct pipeline needs
// roughly one sweep per level of alternating All/Any nesting, because each
// sweep only unwraps one, so a tree deeper than the cap returns correct but
// not fully optimized — and silently, since a partly-optimized plan decides
// exactly what a fully optimized one would. The cap is there for the other
// case: a pass that never converges must not hang route construction.
//
// Convergence is detected by rendering rather than by comparing, because a Node
// is not comparable — a Leaf holds a []factProviderConfig and its Source may
// wrap an fs.FS, so == does not compile and reflect.DeepEqual would descend
// into whatever a filesystem value happens to contain. Explain cannot tell two
// distinct fs sources apart, and that direction of imprecision is safe: a false
// "equal" only stops optimizing early. Deciding that two leaves may be *merged*
// tolerates no such thing, which is why dedupe asks key() instead.
//
// An unknown pass name panics, like every other route-construction mistake in
// this package: a mistyped name is a pipeline nobody asked for, with nothing
// downstream to notice. Names are resolved before any rewriting starts, so a
// bad name late in the list cannot leave a half-run pipeline behind.
func Optimize(n Node, pipeline []string) Node {
	passes := make([]optimizer, 0, len(pipeline))
	for _, name := range pipeline {
		pass, ok := optimizerRegistry[name]
		if !ok {
			panic(fmt.Sprintf("unknown policy optimizer pass %q", name))
		}
		passes = append(passes, pass)
	}
	if len(passes) == 0 {
		// No optimization means none: no rewriting, and no rendering to detect
		// that nothing was rewritten.
		return n
	}

	current := n
	rendered := Explain(current)
	for iteration := 0; iteration < optimizeFixpointLimit; iteration++ {
		for _, pass := range passes {
			current = pass(current)
		}
		next := Explain(current)
		if next == rendered {
			break
		}
		rendered = next
	}
	return current
}

// mapChildren applies a rewrite to a node's children, which is what makes every
// pass in this file bottom-up: a pass calls it first and then rewrites the node
// it is looking at, so it sees children that have already settled.
//
// A Leaf is returned as-is rather than rebuilt. No pass here constructs a Leaf,
// so Source, Facts and MaxBody survive by construction rather than by each pass
// remembering to copy them.
func mapChildren(n Node, rewrite func(Node) Node) Node {
	switch node := n.(type) {
	case AllNode:
		return AllNode{Children: rewriteChildren(node.Children, rewrite)}
	case AnyNode:
		return AnyNode{Children: rewriteChildren(node.Children, rewrite)}
	}
	return n
}

func rewriteChildren(children []Node, rewrite func(Node) Node) []Node {
	rewritten := make([]Node, len(children))
	for index, child := range children {
		rewritten[index] = rewrite(child)
	}
	return rewritten
}

// flatten merges a node's same-kind children into it: All(a, All(b, c)) becomes
// All(a, b, c). Legal because min and max are associative.
//
// Only within one kind. All(a, Any(b, c)) stays as it is: no distributive law
// is available here, and absorbing the disjunction would turn "b or c" into "b
// and c".
func flatten(n Node) Node {
	n = mapChildren(n, flatten)
	switch node := n.(type) {
	case AllNode:
		return AllNode{Children: flattenChildren(node.Children, func(child Node) ([]Node, bool) {
			inner, ok := child.(AllNode)
			return inner.Children, ok
		})}
	case AnyNode:
		return AnyNode{Children: flattenChildren(node.Children, func(child Node) ([]Node, bool) {
			inner, ok := child.(AnyNode)
			return inner.Children, ok
		})}
	}
	return n
}

// flattenChildren returns a new child list with the children of same-kind
// children spliced in. sameKind reports whether a child is the parent's kind
// and, if so, what its children are.
func flattenChildren(children []Node, sameKind func(Node) ([]Node, bool)) []Node {
	merged := make([]Node, 0, len(children))
	for _, child := range children {
		inner, ok := sameKind(child)
		if !ok {
			merged = append(merged, child)
			continue
		}
		if len(inner) == 0 {
			// Deliberately not spliced. A childless composite is malformed
			// input, and splicing its zero children would delete the evidence:
			// All(a, AllNode{}) would silently become All(a), and
			// All(AllNode{}) would leave the parent childless as well. Either
			// way a named startup error from compileChildren becomes a plan
			// that is wrong with nothing to report it, so pass it through and
			// let Compile refuse it.
			merged = append(merged, child)
			continue
		}
		merged = append(merged, inner...)
	}
	return merged
}

// collapseUnary rewrites All(a) and Any(a) to a. Legal because a fold over a
// single element is that element, for either combinator: min(identity, a) and
// max(identity, a) are both a.
func collapseUnary(n Node) Node {
	n = mapChildren(n, collapseUnary)
	switch node := n.(type) {
	case AllNode:
		if len(node.Children) == 1 {
			return node.Children[0]
		}
	case AnyNode:
		if len(node.Children) == 1 {
			return node.Children[0]
		}
	}
	return n
}

// dedupe removes repeated identical leaves from a node's children. Legal
// because min and max are idempotent: min(a, a) = a and max(a, a) = a, whatever
// a is — including truthError, so a leaf that fails is no more informative for
// having failed twice.
func dedupe(n Node) Node {
	n = mapChildren(n, dedupe)
	switch node := n.(type) {
	case AllNode:
		return AllNode{Children: dedupeChildren(node.Children)}
	case AnyNode:
		return AnyNode{Children: dedupeChildren(node.Children)}
	}
	return n
}

// dedupeChildren keeps the first occurrence of each leaf key and drops later
// ones. It always keeps that first occurrence, so it cannot empty a composite:
// the output is shorter than the input only when the input had at least two
// children.
//
// Identity comes from Leaf.key(), never from Explain. Explain renders a source
// with no stable identity as its Go type, so two distinct filesystems sharing a
// path render identically — good enough to stop optimizing early, and a silent
// authorization change if it decided a merge. key() returns "" for exactly
// those leaves, and "" is never equal to anything, including itself. It also
// quotes what it joins, so a separator inside a query or a source name cannot
// make two different leaves look like one.
func dedupeChildren(children []Node) []Node {
	seen := make(map[string]bool, len(children))
	kept := make([]Node, 0, len(children))
	for _, child := range children {
		leaf, ok := child.(Leaf)
		if !ok {
			// Only leaves are compared. Two structurally identical subtrees are
			// left alone: proving them equal means proving every leaf under
			// them equal, and nothing here needs that yet.
			kept = append(kept, child)
			continue
		}
		key := leaf.key()
		if key == "" {
			// No stable identity: never equal to anything, so never merged and
			// never recorded as something a later leaf could be merged into.
			kept = append(kept, child)
			continue
		}
		if seen[key] {
			continue
		}
		seen[key] = true
		kept = append(kept, child)
	}
	return kept
}
