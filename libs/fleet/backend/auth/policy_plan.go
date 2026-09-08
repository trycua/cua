package auth

import (
	"context"
	"errors"
	"fmt"
	"log/slog"

	"github.com/open-policy-agent/opa/rego"
)

// truth is a three-valued logic value ordered truthFalse < truthError <
// truthTrue. All is min over this order and Any is max, so both are
// commutative, associative and idempotent. That is precisely what makes
// optimizer reordering sound; see the design doc.
type truth uint8

// The three values a policy can evaluate to. truthError means the policy could
// not be decided, not that it denied: it sits between the two definite answers
// so that a definite answer from a sibling outranks it in either direction.
const (
	truthFalse truth = iota
	truthError
	truthTrue
)

// Compile-time assertions on the lattice. The order truthFalse < truthError <
// truthTrue is not read by any code today — the combinators below encode
// min/max through their annihilator and identity. These fail to compile if the
// const block is reordered, so the documented lattice cannot silently drift
// from the executable one, and they become load-bearing the moment a pass
// compares truth values directly.
//
// The subtraction is what overflows; the uint conversion is belt-and-braces so
// the assertions keep working if truth ever becomes a signed type, where a
// negative difference would otherwise be legal.
const (
	_ = uint(truthError - truthFalse)
	_ = uint(truthTrue - truthError)

	// truthFalse must be the zero value. A verdict{} left unfilled by some
	// future code path has to read as a deny with no error, so that forgetting
	// to set a truth fails closed rather than open.
	_ = uint(0 - truthFalse)
)

// verdict is the result of evaluating a policy against one request.
type verdict struct {
	truth  truth
	err    error  // non-nil if and only if truth == truthError
	reason string // non-empty only when truth == truthFalse
}

// A combinator is a fold over truth: All is min, Any is max. annihilator must
// be truthFalse or truthTrue, never truthError — fold's switch matches the
// annihilator before the error arm, so an error annihilator would shadow it
// and errors would stop being remembered. The two values below are the only
// combinators that exist, which is what keeps that precondition true.
type combinator struct{ annihilator, identity truth }

var (
	conjunction = combinator{annihilator: truthFalse, identity: truthTrue} // min
	disjunction = combinator{annihilator: truthTrue, identity: truthFalse} // max
)

type compiledNode interface {
	eval(context.Context, *requestPolicyInput) verdict
	// bodyBudget is the largest raw-body limit in this subtree. Reading the
	// request once against the root's budget makes each leaf's own limit check
	// a pure function of (len(body), leaf.MaxBody), independent of evaluation
	// order. Answering it here rather than in a switch over Node makes the
	// question unanswerable-by-omission: a node kind that does not implement
	// it does not satisfy compiledNode and does not compile.
	bodyBudget() int64
}

// CompiledPolicy is an executable plan produced from an optimized AST.
type CompiledPolicy struct {
	root       compiledNode
	bodyBudget int64
}

// eval runs the plan against one request. It is the plan's whole execution
// surface; nothing outside this file should reach for root.
func (policy *CompiledPolicy) eval(ctx context.Context, input *requestPolicyInput) verdict {
	return policy.root.eval(ctx, input)
}

// Compile turns a policy AST into an executable plan. It performs the source
// I/O and the Rego compilation that Policy deliberately defers.
func Compile(n Node) (*CompiledPolicy, error) {
	root, err := compileNode(n)
	if err != nil {
		return nil, err
	}
	return &CompiledPolicy{root: root, bodyBudget: root.bodyBudget()}, nil
}

func maxChildBudget(children []compiledNode) int64 {
	var budget int64
	for _, child := range children {
		if candidate := child.bodyBudget(); candidate > budget {
			budget = candidate
		}
	}
	return budget
}

func compileNode(n Node) (compiledNode, error) {
	switch node := n.(type) {
	case Leaf:
		return compileLeaf(node)
	case AllNode:
		children, err := compileChildren(node, node.Children)
		if err != nil {
			return nil, err
		}
		return &compiledAll{children: children}, nil
	case AnyNode:
		children, err := compileChildren(node, node.Children)
		if err != nil {
			return nil, err
		}
		return &compiledAny{children: children}, nil
	case BecauseNode:
		if node.Child == nil {
			return nil, fmt.Errorf("policy reason %q has no child", node.Reason)
		}
		if node.Reason == "" {
			return nil, fmt.Errorf("policy reason is empty")
		}
		child, err := compileNode(node.Child)
		if err != nil {
			return nil, err
		}
		return &compiledBecause{child: child, reason: node.Reason}, nil
	default:
		return nil, fmt.Errorf("cannot compile unknown policy node %T", n)
	}
}

func compileChildren(parent Node, nodes []Node) ([]compiledNode, error) {
	// A childless composite folds to its identity, which for a conjunction is
	// an unconditional allow. All() and Any() reject zero arguments, but
	// Children is an exported field and rewrite passes build these values
	// directly, so a pass that filtered a conjunction down to nothing must
	// fail here rather than authorize every request on the route.
	if len(nodes) == 0 {
		return nil, fmt.Errorf("policy node %T has no children", parent)
	}
	compiled := make([]compiledNode, 0, len(nodes))
	for _, child := range nodes {
		node, err := compileNode(child)
		if err != nil {
			return nil, err
		}
		compiled = append(compiled, node)
	}
	return compiled, nil
}

func compileLeaf(leaf Leaf) (compiledNode, error) {
	// Policy rejects a nil source, but it is not the only way to build a
	// Leaf: rewrite passes construct them directly, and one that rebuilt a
	// Leaf from parts and dropped Source must get an error, not a panic.
	if leaf.Source == nil {
		return nil, fmt.Errorf("policy %q has no source", leaf.Query)
	}
	modules, err := leaf.Source.modules()
	if err != nil {
		return nil, fmt.Errorf("load modules for policy %q: %w", leaf.Query, err)
	}
	options := []func(*rego.Rego){rego.Query(leaf.Query)}
	for _, module := range modules {
		options = append(options, rego.Module(module.name, module.source))
	}
	prepared, err := rego.New(options...).PrepareForEval(context.Background())
	if err != nil {
		return nil, fmt.Errorf("prepare OPA policy %q: %w", leaf.Query, err)
	}
	return &compiledLeaf{
		query: prepared,
		config: policyConfig{
			query:         leaf.Query,
			factProviders: leaf.Facts,
			maxBodyBytes:  leaf.MaxBody,
		},
	}, nil
}

type compiledLeaf struct {
	query  rego.PreparedEvalQuery
	config policyConfig
}

func (leaf *compiledLeaf) eval(ctx context.Context, input *requestPolicyInput) verdict {
	document, err := input.forPolicy(ctx, leaf.config)
	if err != nil {
		return verdict{truth: truthError, err: err}
	}
	result, err := leaf.query.Eval(ctx, rego.EvalInput(document))
	if err != nil {
		return verdict{truth: truthError, err: err}
	}
	if result.Allowed() {
		return verdict{truth: truthTrue}
	}
	return verdict{truth: truthFalse}
}

func (leaf *compiledLeaf) bodyBudget() int64 { return leaf.config.maxBodyBytes }

type compiledBecause struct {
	child  compiledNode
	reason string
}

func (node *compiledBecause) eval(ctx context.Context, input *requestPolicyInput) verdict {
	result := node.child.eval(ctx, input)
	if result.truth == truthFalse {
		result.reason = node.reason
		return result
	}
	result.reason = ""
	return result
}

func (node *compiledBecause) bodyBudget() int64 { return node.child.bodyBudget() }

type compiledAll struct{ children []compiledNode }

func (node *compiledAll) eval(ctx context.Context, input *requestPolicyInput) verdict {
	return fold(ctx, input, node.children, conjunction)
}

func (node *compiledAll) bodyBudget() int64 { return maxChildBudget(node.children) }

type compiledAny struct{ children []compiledNode }

func (node *compiledAny) eval(ctx context.Context, input *requestPolicyInput) verdict {
	return fold(ctx, input, node.children, disjunction)
}

func (node *compiledAny) bodyBudget() int64 { return maxChildBudget(node.children) }

// fold evaluates children left to right, short-circuiting on the annihilating
// value only. Errors are remembered rather than aborting, so a later
// annihilator can still win — that is what makes the fold order-independent.
func fold(ctx context.Context, input *requestPolicyInput, children []compiledNode, over combinator) verdict {
	var errs []error
	var reason string
	for _, child := range children {
		result := child.eval(ctx, input)
		switch result.truth {
		case over.annihilator:
			if len(errs) > 0 {
				// The lattice drops these on purpose: an annihilator decides
				// the fold whatever its siblings did. Record only reviewed
				// classification fields because provider causes may contain
				// credentials or request-derived data.
				joined := errors.Join(errs...)
				class := "discarded_policy_error"
				retryable := false
				factNamespace := ""
				var factErr *FactUnavailableError
				if errors.As(joined, &factErr) {
					class = "discarded_dependency_unavailable"
					retryable = true
					factNamespace = factErr.Namespace
				}
				slog.Warn("opa: policy error discarded; a sibling decided the verdict",
					"allowed", over.annihilator == truthTrue,
					"class", class,
					"retryable", retryable,
					"facts", factNamespace)
			}
			return verdict{truth: over.annihilator, reason: result.reason}
		case truthError:
			errs = append(errs, result.err)
		case truthFalse:
			if reason == "" {
				reason = result.reason
			}
		}
	}
	if len(errs) > 0 {
		return verdict{truth: truthError, err: errors.Join(errs...)}
	}
	return verdict{truth: over.identity, reason: reason}
}
