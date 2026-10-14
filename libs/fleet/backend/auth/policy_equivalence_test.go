package auth

import (
	"errors"
	"fmt"
	"math/rand"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// This file is the oracle the optimizer is measured against. It compares the
// real evaluator with a naive reference interpreter over randomly generated
// policy trees, so that a pass which reorders leaves has something to be wrong
// against.
//
// The reference is total: it visits every child and folds with min (All) and
// max (Any). The real evaluator short-circuits on the annihilating value. The
// two nonetheless agree on the *set* of errors, for a reason worth stating
// because the whole harness rests on it: a verdict carries errors only when its
// truth is truthError, and a fold reaches truthError only when no child
// returned the annihilator — which means the loop ran to completion and visited
// every child. So whenever errors are observable at all, the lazy evaluator saw
// exactly the set the total one did. When an annihilator does win, both throw
// the errors away.
//
// The file reads in dependency order: the reference model, then the AST the
// model is turned into, then the generator, then the comparison harness, then
// the tests that drive it.

// ---------------------------------------------------------------------------
// Reference model
//
// The specification. Everything here is written to be checkable by reading it,
// and deliberately shares no code with the evaluator it judges.
// ---------------------------------------------------------------------------

// refTruth is the reference lattice, ordered refFalse < refError < refTrue.
// It is spelled out here rather than reusing the evaluator's truth type on
// purpose: an oracle that imports its subject's definition of the answer can
// only ever confirm it. evaluatorTruth is the one place the two representations
// are allowed to meet.
type refTruth uint8

const (
	refFalse refTruth = iota
	refError
	refTrue
)

// Compile-time assertions on the reference lattice, mirroring the ones truth
// carries in policy_plan.go. refCombine is min and max over this order, so the
// order is not decoration: reorder the block above and the oracle quietly
// becomes a different specification, one that would then accuse the evaluator
// of the oracle's own bug. The subtraction is what overflows; the uint
// conversion keeps the assertions working if refTruth ever becomes signed.
const (
	_ = uint(refError - refFalse)
	_ = uint(refTrue - refError)

	// refFalse must be the zero value, so that a refVerdict{} left unfilled
	// reads as a deny rather than an allow.
	_ = uint(0 - refFalse)
)

// evaluatorTruth maps a reference value onto the evaluator's. The switch is
// exhaustive with a panicking default so that adding a value to either lattice
// is a change someone has to make here on purpose, rather than one that
// silently lands on truthError and makes the comparison agree by accident.
func (value refTruth) evaluatorTruth() truth {
	switch value {
	case refFalse:
		return truthFalse
	case refError:
		return truthError
	case refTrue:
		return truthTrue
	default:
		panic(fmt.Sprintf("no evaluator truth for reference truth %d", uint8(value)))
	}
}

// behavior is a scripted leaf outcome. Each one exercises a distinct path into
// a verdict: two definite answers, the two independent ways a leaf becomes
// undecidable (fact loading and Rego evaluation), and the two body outcomes.
//
// Every switch over a behavior in this file is exhaustive and panics on an
// unknown one. That is load-bearing rather than tidy: a new behavior absorbed
// by a default arm would be scripted by the oracle and by the AST builder as
// the same thing, so the two would agree about a leaf nobody implemented and
// the per-behavior test would report success for it.
type behavior uint8

const (
	behaviorAllow behavior = iota
	behaviorDeny
	behaviorFactError
	behaviorRegoError
	behaviorBodyOK
	behaviorBodyOverLimit
	behaviorCount
)

// sourceVariant selects which Rego module a leaf's behavior is scripted in.
// The reference evaluator ignores it entirely — a leaf's verdict comes from its
// behavior and nothing else — so it changes only the shape of the AST the
// evaluator is handed.
//
// It exists because of a measured hole. The first version of this generator
// derived a leaf's module, its query, its facts and its body limit all from the
// behavior, which made source ↔ (query, facts, MaxBody) a bijection: no two
// generated leaves could share a module while differing in anything else.
// Mutating dedupeChildren to key on Leaf.Source.sourceKey() alone — an unsound
// merge, since leaves sharing a module can ask different questions of it — left
// all 300 seeds green. variantSharedSource is the shape that was missing.
type sourceVariant uint8

const (
	// variantOwnSource gives each behavior a module of its own, so a source
	// implies the rest of the leaf. The zero value, and the only variant the
	// 300-seed corpus draws, which is what keeps those trees exactly what they
	// were.
	variantOwnSource sourceVariant = iota

	// variantSharedSource puts every behavior in one module, so two leaves
	// drawn this way share a source and differ only in the query they name, the
	// facts they carry or the body limit they declare. Every one of those three
	// axes has a pair that differs in it alone, which is what a dedupe keyed on
	// any proper subset of Leaf.key() gets wrong:
	//
	//	query only:  behaviorAllow vs behaviorRegoError
	//	facts only:  behaviorDeny  vs behaviorFactError
	//	body only:   behaviorAllow vs behaviorBodyOverLimit
	//
	// The last two are the narrow ones — those pairs agree on source and query
	// and differ in a single field of the leaf.
	variantSharedSource

	sourceVariantCount
)

type refKind uint8

const (
	refLeaf refKind = iota
	refAll
	refAny
)

type refTree struct {
	kind   refKind
	leaf   behavior
	source sourceVariant
	kids   []refTree
}

// refVerdict is a reference result. bodyErr and otherErr track which kinds of
// error survive, because the middleware maps a surviving body error to 400 and
// anything else to 500.
type refVerdict struct {
	truth    refTruth
	bodyErr  bool
	otherErr bool
}

func (result refVerdict) status() int {
	switch result.truth {
	case refTrue:
		return http.StatusNoContent
	case refFalse:
		return http.StatusForbidden
	case refError:
		if result.bodyErr {
			return http.StatusBadRequest
		}
		return http.StatusInternalServerError
	default:
		panic(fmt.Sprintf("no status for reference truth %d", uint8(result.truth)))
	}
}

// verdict is the reference evaluator. It never short-circuits: every child is
// visited and combined, so the result is a plain fold of min (All) or max (Any)
// over the lattice and is order-independent by construction rather than by
// argument.
func (tree refTree) verdict() refVerdict {
	if tree.kind == refLeaf {
		switch tree.leaf {
		case behaviorAllow, behaviorBodyOK:
			return refVerdict{truth: refTrue}
		case behaviorDeny:
			return refVerdict{truth: refFalse}
		case behaviorFactError, behaviorRegoError:
			return refVerdict{truth: refError, otherErr: true}
		case behaviorBodyOverLimit:
			return refVerdict{truth: refError, bodyErr: true}
		default:
			panic(fmt.Sprintf("reference evaluator scripts no verdict for behavior %d", uint8(tree.leaf)))
		}
	}

	result := refVerdict{truth: refTrue}
	if tree.kind == refAny {
		result = refVerdict{truth: refFalse}
	}
	for _, kid := range tree.kids {
		result = refCombine(tree.kind, result, kid.verdict())
	}
	return result
}

// refCombine is min for All and max for Any, carrying the union of both
// operands' error kinds. The union is dropped whenever the result is not
// refError, which is what makes an annihilator discard its siblings' errors —
// the same thing the real fold does, arrived at without a special case.
func refCombine(kind refKind, left, right refVerdict) refVerdict {
	combined := refVerdict{
		bodyErr:  left.bodyErr || right.bodyErr,
		otherErr: left.otherErr || right.otherErr,
	}
	if kind == refAll {
		combined.truth = min(left.truth, right.truth)
	} else {
		combined.truth = max(left.truth, right.truth)
	}
	if combined.truth != refError {
		combined.bodyErr, combined.otherErr = false, false
	}
	return combined
}

// ---------------------------------------------------------------------------
// AST construction
//
// Turning a scripted tree into the real Node the evaluator runs, and building
// the request it runs against.
// ---------------------------------------------------------------------------

const (
	// equivalenceRequestBody is the body every generated request carries.
	equivalenceRequestBody = "0123456789"

	// The two raw-body limits the body behaviors use. Wide sits above the
	// request body and narrow below it, which is the whole reason
	// behaviorBodyOK allows and behaviorBodyOverLimit errors. Because the plan
	// reads once against the widest limit in the tree, each leaf's own limit
	// then decides its own outcome, so neither behavior depends on which of
	// them ran first.
	equivalenceWideBodyLimit   = 1024
	equivalenceNarrowBodyLimit = 2
)

// The relationship the comment above describes, made executable. Retuning
// either limit past the request body silently turns a body behavior into its
// opposite, and every expectation in the reference would follow it.
const (
	_ = uint(equivalenceWideBodyLimit - len(equivalenceRequestBody))
	_ = uint(len(equivalenceRequestBody) - equivalenceNarrowBodyLimit - 1)
)

// equivalenceModule is the Rego every generated leaf runs: one rule per
// behavior, in whichever package the leaf's sourceVariant asks for.
//
// One template rather than a module per behavior, because a leaf's source has to
// be free to vary independently of its behavior. If a module contained only the
// rule its leaf needs, the module would give the behavior away and two leaves
// could never share one while asking different questions of it — the hole
// sourceVariant documents.
//
// Only the rule a leaf's query names is evaluated, which is why the conflicting
// pair can sit in every package without spoiling any other behavior scripted
// beside it. behaviorRegoError depends on that conflict being an
// evaluation-time error rather than a compile-time one: it is the one path to
// truthError that a broken fact provider does not cover, and a module that
// failed to compile would take every behavior in it down instead.
func equivalenceModule(pkg string) string {
	return fmt.Sprintf(`package %s
allow { true }
needsfacts { input.facts.u.ok }
conflict = 1 { true }
conflict = 2 { true }
conflicted { conflict == 1 }
bodymatch { input.body == %q }
`, pkg, equivalenceRequestBody)
}

// packageName is the module a leaf of this variant puts its behavior in.
func (variant sourceVariant) packageName(scripted behavior) string {
	switch variant {
	case variantOwnSource:
		return fmt.Sprintf("eq%d", scripted)
	case variantSharedSource:
		return "eqshared"
	default:
		panic(fmt.Sprintf("no package scripted for source variant %d", uint8(variant)))
	}
}

// leafPolicy is what a behavior is made of: the rule that decides it, and the
// leaf configuration it needs to decide that way. Which module the rule lives in
// is deliberately not here — that is the sourceVariant's business, and keeping
// the two apart is what lets a source and a behavior vary independently.
type leafPolicy struct {
	rule    string
	options []PolicyOption
}

func (scripted behavior) policy() leafPolicy {
	switch scripted {
	case behaviorAllow:
		return leafPolicy{rule: "allow"}
	case behaviorDeny:
		// needsfacts with nothing to load. input.facts.u.ok is then an
		// undefined reference, which makes the rule undefined and the leaf a
		// deny — an undefined path in Rego is not an error.
		//
		// Deliberately the same rule as behaviorFactError, and deliberately
		// without its provider: under variantSharedSource these two agree on
		// source, on query and on MaxBody, and differ in Facts alone. That is
		// the third axis a leaf can differ on, and until this pairing existed
		// nothing generated could reach it — behaviorFactError was the only
		// scripted leaf carrying a provider and also the only one naming this
		// rule, so a facts difference never appeared without a query difference
		// beside it. A dedupe keyed on everything but Facts merges these two,
		// and 403 becomes 500 or the reverse.
		return leafPolicy{rule: "needsfacts"}
	case behaviorFactError:
		return leafPolicy{rule: "needsfacts", options: []PolicyOption{
			WithFacts("u", &testFactProvider{
				cacheKey: "equivalence-broken",
				err:      errors.New("database unavailable"),
			}),
		}}
	case behaviorRegoError:
		return leafPolicy{rule: "conflicted"}
	case behaviorBodyOK:
		return leafPolicy{rule: "bodymatch", options: []PolicyOption{WithRawBody(equivalenceWideBodyLimit)}}
	case behaviorBodyOverLimit:
		// Deliberately the same rule as behaviorAllow. Under
		// variantSharedSource these two leaves then agree on source and on
		// query and differ in MaxBody alone, so a dedupe that ignored MaxBody
		// would merge a leaf that allows with one that must 400.
		return leafPolicy{rule: "allow", options: []PolicyOption{WithRawBody(equivalenceNarrowBodyLimit)}}
	default:
		panic(fmt.Sprintf("no policy scripted for behavior %d", uint8(scripted)))
	}
}

// astNode builds the real AST. Two leaves are structurally identical exactly
// when they share a behavior and a source variant, which is what a dedupe pass
// has to merge — and nothing weaker, which is what it has to refuse.
func (tree refTree) astNode() Node {
	if tree.kind != refLeaf {
		kids := make([]Node, 0, len(tree.kids))
		for _, kid := range tree.kids {
			kids = append(kids, kid.astNode())
		}
		if tree.kind == refAll {
			return All(kids...)
		}
		return Any(kids...)
	}

	pkg := tree.source.packageName(tree.leaf)
	scripted := tree.leaf.policy()
	options := append(
		[]PolicyOption{Query(fmt.Sprintf("data.%s.%s", pkg, scripted.rule))},
		scripted.options...,
	)
	return Policy(Inline(pkg+".rego", equivalenceModule(pkg)), options...)
}

// newEquivalenceRequest builds the request every comparison runs against. The
// harness evaluates each tree twice, through Compile and through
// PolicyMiddleware, and the two must see the same request: two construction
// sites that have to agree is the desync harnessMode exists to prevent, and
// there is no reason to leave it unprevented for the request.
func newEquivalenceRequest() *http.Request {
	return httptest.NewRequest(http.MethodPost, "/", strings.NewReader(equivalenceRequestBody))
}

// ---------------------------------------------------------------------------
// Generator
// ---------------------------------------------------------------------------

const (
	// equivalenceSeeds is the fixed seed range. Fixed rather than time-seeded
	// so a failure is reproducible from the seed printed in the message alone.
	equivalenceSeeds = 300

	// equivalenceDepth bounds generated trees. The generator and the assertion
	// that every depth appears both read it, so the two cannot drift.
	equivalenceDepth = 4
)

// drawSource is where the generator's decisions come from. The seed corpus
// draws from math/rand; the fuzz target in policy_fuzz_test.go draws from the
// fuzzer's byte string. One generator behind both is what makes a shape
// reachable by one reachable by the other.
type drawSource interface {
	// intn returns a value in [0, n) for a positive n.
	intn(n int) int
}

// randDraws is the math/rand implementation, and the only one the fixed seed
// range uses.
type randDraws struct{ rng *rand.Rand }

func (source randDraws) intn(n int) int { return source.rng.Intn(n) }

// genConfig is the whole of what the seed corpus and the fuzz target disagree
// about.
type genConfig struct {
	depth int

	// sourceVariants bounds the sourceVariant values leaves may be drawn from.
	// One means variantOwnSource and — this is the part that matters — no draw
	// at all, so the fixed seed range consumes exactly the random stream it
	// consumed before variants existed and the floors in
	// TestEquivalenceGeneratorProducesDistinctTrees still describe the same 300
	// trees.
	sourceVariants int
}

// genTree builds a random tree of at most the given depth from the fixed seed
// range's generator: one source variant, so a leaf is its behavior.
func genTree(rng *rand.Rand, depth int) refTree {
	return genTreeFrom(randDraws{rng: rng}, genConfig{depth: depth, sourceVariants: 1})
}

// genTreeFrom builds a random tree. The root is always a composite. A bare leaf
// has nothing for an optimizer to get wrong, and with only six behaviors it
// collapses whatever fraction of the draws produce it onto six trees —
// measured, that cost a third of the seeds and a third of the distinct shapes.
// TestReferenceMatchesEveryLeafBehavior covers leaves alone.
func genTreeFrom(draw drawSource, config genConfig) refTree {
	return genComposite(draw, config, config.depth)
}

func genComposite(draw drawSource, config genConfig, depth int) refTree {
	kind := refAll
	if draw.intn(2) == 0 {
		kind = refAny
	}
	kids := make([]refTree, 1+draw.intn(3))
	for index := range kids {
		kids[index] = genSubtree(draw, config, depth-1)
	}
	return refTree{kind: kind, kids: kids}
}

func genSubtree(draw drawSource, config genConfig, depth int) refTree {
	if depth == 0 || draw.intn(3) == 0 {
		return genLeaf(draw, config)
	}
	return genComposite(draw, config, depth)
}

func genLeaf(draw drawSource, config genConfig) refTree {
	leaf := refTree{kind: refLeaf, leaf: behavior(draw.intn(int(behaviorCount)))}
	if config.sourceVariants > 1 {
		leaf.source = sourceVariant(draw.intn(config.sourceVariants))
	}
	return leaf
}

// signature is a structural identity for a generated tree, used to prove the
// generator is not emitting the same handful of trees under every seed.
func (tree refTree) signature() string {
	var builder strings.Builder
	tree.signatureInto(&builder)
	return builder.String()
}

func (tree refTree) signatureInto(builder *strings.Builder) {
	if tree.kind == refLeaf {
		fmt.Fprintf(builder, "L%d", tree.leaf)
		if tree.source != variantOwnSource {
			// Only when it is not the default, so that the fixed seed range's
			// signatures — and the distinct-tree floor counted from them — are
			// the strings they were before variants existed. A leaf's source
			// still has to appear, because two leaves differing only in it
			// build different ASTs.
			fmt.Fprintf(builder, "@%d", tree.source)
		}
		return
	}
	if tree.kind == refAll {
		builder.WriteString("All(")
	} else {
		builder.WriteString("Any(")
	}
	for index, kid := range tree.kids {
		if index > 0 {
			builder.WriteString(",")
		}
		kid.signatureInto(builder)
	}
	builder.WriteString(")")
}

func treeDepth(tree refTree) int {
	deepest := 0
	for _, kid := range tree.kids {
		if depth := treeDepth(kid) + 1; depth > deepest {
			deepest = depth
		}
	}
	return deepest
}

func collectVariety(tree refTree, kinds map[refKind]bool, leaves map[behavior]bool) {
	if tree.kind == refLeaf {
		leaves[tree.leaf] = true
		return
	}
	kinds[tree.kind] = true
	for _, kid := range tree.kids {
		collectVariety(kid, kinds, leaves)
	}
}

// ---------------------------------------------------------------------------
// Comparison harness
// ---------------------------------------------------------------------------

// harnessMode says how the plan under test is built. The comparison looks at
// the plan twice — once through Compile for the verdict and once through
// PolicyMiddleware for the status — and those two have to be the same plan. If
// the verdict half tested an unoptimized tree while the status half tested an
// optimized one, the comparison would still be green and would be comparing
// nothing.
type harnessMode struct {
	plan    func(Node) Node
	options []MiddlewareOption
}

// modeRunning derives both halves of a mode from one list of pass names, so
// they cannot disagree. Written as two literals, "these options reproduce that
// rewrite" was a claim in a comment; here it is the only thing either half is
// built from.
func modeRunning(passes ...string) harnessMode {
	return harnessMode{
		plan:    func(node Node) Node { return Optimize(node, passes) },
		options: []MiddlewareOption{WithPipeline(passes...)},
	}
}

var (
	unoptimizedMode = modeRunning()
	optimizedMode   = modeRunning(DefaultPipeline()...)
)

func runPolicyStatus(t *testing.T, node Node, options ...MiddlewareOption) int {
	t.Helper()
	handler := PolicyMiddleware(node, options...)(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, newEquivalenceRequest())
	return response.Code
}

// errorKinds splits a verdict's error into the two classes the middleware
// distinguishes. It walks the whole error graph rather than asking errors.As
// twice, because errors.As can only answer "is there a body error in here" and
// the harness also needs "is there anything else in here" — otherwise a fold
// that dropped every non-body error would still look right.
//
// Both unwrap shapes have to be followed. A join gives Unwrap() []error and a
// %w chain gives Unwrap() error, and stopping at a single-error wrapper would
// hand the rest of the question to errors.As, which reports the first body
// error it finds and says nothing about what sits beside it. Nothing in the
// evaluator wraps today — fold only joins, and leaves return raw errors — but
// the cost of being wrong here is the classifier silently agreeing with a
// misclassification it exists to catch, so it does not depend on that.
//
// An unwrap that yields nothing is not evidence of nothing. Implementing the
// interface is not a promise of a cause: OPA's *topdown.Error has Unwrap()
// error and returns nil from it, so descending unconditionally would classify
// every Rego evaluation error as no error at all. Both arms therefore fall
// through to the leaf handling below when they come back empty.
func errorKinds(err error) (bodyErr, otherErr bool) {
	switch unwrappable := err.(type) {
	case nil:
		return false, false
	case interface{ Unwrap() []error }:
		if children := unwrappable.Unwrap(); len(children) > 0 {
			for _, child := range children {
				childBody, childOther := errorKinds(child)
				bodyErr, otherErr = bodyErr || childBody, otherErr || childOther
			}
			return bodyErr, otherErr
		}
	case interface{ Unwrap() error }:
		// Annotation, not a second failure: a wrapped body error is still one
		// body error, so this reports the cause rather than counting the
		// wrapper as an "other". Classifying the wrapper separately would make
		// every leaf that gained a context string look like two errors and put
		// the reference's per-behavior flags permanently out of step.
		if cause := unwrappable.Unwrap(); cause != nil {
			return errorKinds(cause)
		}
	}
	var body policyBodyError
	if errors.As(err, &body) {
		return true, false
	}
	return false, true
}

// assertMatchesReference is the differential comparison. It checks the plan's
// verdict — truth, and which kinds of error survived — and then the status the
// middleware derives from it, because a harness that only compared statuses
// would let three distinct verdicts collapse onto 500.
func assertMatchesReference(t *testing.T, mode harnessMode, label string, tree refTree) {
	t.Helper()
	want := tree.verdict()
	node := mode.plan(tree.astNode())

	compiled, err := Compile(node)
	if err != nil {
		t.Fatalf("%s: compile: %v\n%s", label, err, Explain(node))
	}
	request := newEquivalenceRequest()
	got := compiled.eval(request.Context(), newRequestPolicyInput(request, compiled.bodyBudget))

	if got.truth != want.truth.evaluatorTruth() {
		t.Errorf("%s: truth = %d, want %d\n%s", label, got.truth, want.truth.evaluatorTruth(), Explain(node))
	}
	// verdict documents err as non-nil if and only if truth is truthError.
	// Nothing else asserts that invariant over generated input.
	if (got.err != nil) != (got.truth == truthError) {
		t.Errorf("%s: truth = %d with err = %v, want an error exactly when undecided\n%s", label, got.truth, got.err, Explain(node))
	}
	gotBodyErr, gotOtherErr := errorKinds(got.err)
	if gotBodyErr != want.bodyErr || gotOtherErr != want.otherErr {
		t.Errorf("%s: error kinds = (body %t, other %t), want (body %t, other %t); err = %v\n%s",
			label, gotBodyErr, gotOtherErr, want.bodyErr, want.otherErr, got.err, Explain(node))
	}
	// A freshly built tree, not the rewritten one: PolicyMiddleware runs the
	// pipeline itself, so handing it mode.plan's output would optimize twice and
	// quietly turn this into a test of idempotence. Rebuilding also means a pass
	// that rewrites in place cannot reach the verdict half above.
	if status := runPolicyStatus(t, tree.astNode(), mode.options...); status != want.status() {
		t.Errorf("%s: status = %d, want %d\n%s", label, status, want.status(), Explain(node))
	}
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// hollowWrapper implements Unwrap() error with no cause, the shape
// *topdown.Error has.
type hollowWrapper struct{ message string }

func (err hollowWrapper) Error() string { return err.message }
func (hollowWrapper) Unwrap() error     { return nil }

// hollowJoin is the same shape in the multi-error arm.
type hollowJoin struct{ message string }

func (err hollowJoin) Error() string { return err.message }
func (hollowJoin) Unwrap() []error   { return nil }

// TestErrorKindsWalksWrappedAndJoinedErrors pins the classifier the whole
// comparison rests on, rather than leaving it to whichever shapes the generated
// trees happen to produce. See errorKinds' doc for why each shape below matters.
func TestErrorKindsWalksWrappedAndJoinedErrors(t *testing.T) {
	body := policyBodyError{message: "request body exceeds policy limit"}
	other := errors.New("database unavailable")

	cases := []struct {
		name         string
		err          error
		wantBody     bool
		wantOtherErr bool
	}{
		{"nil", nil, false, false},
		{"bare body error", body, true, false},
		{"bare other error", other, false, true},
		{"join of both", errors.Join(body, other), true, true},
		{"nested joins", errors.Join(errors.Join(body), errors.Join(other)), true, true},
		{"wrapped body error", fmt.Errorf("leaf: %w", body), true, false},
		{"wrapped other error", fmt.Errorf("leaf: %w", other), false, true},
		{"wrapped join of both", fmt.Errorf("subtree: %w", errors.Join(body, other)), true, true},
		{"join containing a wrapped body error", errors.Join(fmt.Errorf("leaf: %w", body), other), true, true},
		{"doubly wrapped join", fmt.Errorf("plan: %w", fmt.Errorf("subtree: %w", errors.Join(body, other))), true, true},
		// Not hypothetical: this is the shape every Rego evaluation error
		// arrives in.
		{"wrapper with no cause", hollowWrapper{message: "eval_conflict_error"}, false, true},
		{"joiner with no children", hollowJoin{message: "empty join"}, false, true},
		{"join beside a wrapper with no cause", errors.Join(body, hollowWrapper{message: "eval_conflict_error"}), true, true},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			gotBody, gotOther := errorKinds(testCase.err)
			if gotBody != testCase.wantBody || gotOther != testCase.wantOtherErr {
				t.Fatalf("errorKinds(%v) = (body %t, other %t), want (body %t, other %t)",
					testCase.err, gotBody, gotOther, testCase.wantBody, testCase.wantOtherErr)
			}
		})
	}
}

func TestReferenceEvaluatorMatchesHandDerivedVerdicts(t *testing.T) {
	leaf := func(scripted behavior) refTree { return refTree{kind: refLeaf, leaf: scripted} }
	allOf := func(kids ...refTree) refTree { return refTree{kind: refAll, kids: kids} }
	anyOf := func(kids ...refTree) refTree { return refTree{kind: refAny, kids: kids} }

	// The reference is the specification, so it gets pinned by hand rather than
	// by agreement with the code it is supposed to judge. Each case states the
	// fold that produces it.
	cases := []struct {
		name string
		tree refTree
		want refVerdict
	}{
		{
			// min(true, false) = false.
			name: "all of allow and deny denies",
			tree: allOf(leaf(behaviorAllow), leaf(behaviorDeny)),
			want: refVerdict{truth: refFalse},
		},
		{
			// max(error{other}, error{body}) = error carrying both.
			name: "any of two error kinds keeps both",
			tree: anyOf(leaf(behaviorFactError), leaf(behaviorBodyOverLimit)),
			want: refVerdict{truth: refError, bodyErr: true, otherErr: true},
		},
		{
			// inner max(false, error{other}) = error{other};
			// outer min(true, error{other}) = error{other}.
			name: "all of body allow and any of deny and fact error is undecided",
			tree: allOf(leaf(behaviorBodyOK), anyOf(leaf(behaviorDeny), leaf(behaviorFactError))),
			want: refVerdict{truth: refError, otherErr: true},
		},
		{
			// left min(error{other}, false) = false;
			// right min(error{body}, true) = error{body};
			// outer max(false, error{body}) = error{body}.
			name: "any of a denying branch and a body-erroring branch keeps the body error",
			tree: anyOf(
				allOf(leaf(behaviorFactError), leaf(behaviorDeny)),
				allOf(leaf(behaviorBodyOverLimit), leaf(behaviorAllow)),
			),
			want: refVerdict{truth: refError, bodyErr: true},
		},
		{
			// inner max(error{body}, true) = true, which discards the body
			// error; outer min(true, false) = false. The discard is the point:
			// without it this would be 400 rather than 403.
			name: "an annihilated body error does not reach the outer fold",
			tree: allOf(anyOf(leaf(behaviorBodyOverLimit), leaf(behaviorAllow)), leaf(behaviorDeny)),
			want: refVerdict{truth: refFalse},
		},
		{
			// max(error{other}, false) = error{other}: no disjunct allowed, so
			// the Rego conflict survives as a 500.
			name: "any of rego error and deny stays undecided",
			tree: anyOf(leaf(behaviorRegoError), leaf(behaviorDeny)),
			want: refVerdict{truth: refError, otherErr: true},
		},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			if got := testCase.tree.verdict(); got != testCase.want {
				t.Fatalf("reference verdict = %+v, want %+v", got, testCase.want)
			}
			// Having pinned the reference by hand, hold the real evaluator to
			// it too, so these stay readable worked examples of the whole
			// comparison and not just of the oracle.
			assertMatchesReference(t, unoptimizedMode, testCase.name, testCase.tree)
		})
	}
}

func TestEquivalenceGeneratorProducesDistinctTrees(t *testing.T) {
	// A differential test over a degenerate generator is the worst kind of
	// green: hundreds of passing seeds proving one tree. This is the floor that
	// keeps the seed range meaningful. The count observed is 278 of 300, so
	// ordinary churn in the generator will not trip this but a collapse to a
	// handful of shapes will.
	const wantAtLeast = 240

	signatures := map[string]bool{}
	depths := map[int]bool{}
	kinds := map[refKind]bool{}
	leaves := map[behavior]bool{}
	statuses := map[int]int{}
	for seed := int64(1); seed <= equivalenceSeeds; seed++ {
		tree := genTree(rand.New(rand.NewSource(seed)), equivalenceDepth)
		signatures[tree.signature()] = true
		depths[treeDepth(tree)] = true
		statuses[tree.verdict().status()]++
		collectVariety(tree, kinds, leaves)
	}

	t.Logf("%d distinct trees over %d seeds; depths %v; %d of %d leaf behaviors; statuses %v",
		len(signatures), equivalenceSeeds, depths, len(leaves), behaviorCount, statuses)
	if len(signatures) < wantAtLeast {
		t.Errorf("distinct trees = %d over %d seeds, want at least %d", len(signatures), equivalenceSeeds, wantAtLeast)
	}
	// Distinctness alone does not prove coverage: a generator could emit
	// hundreds of distinct all-allow conjunctions. Every leaf behavior, both
	// composite kinds and every depth up to the limit must actually appear.
	if len(leaves) != int(behaviorCount) {
		t.Errorf("leaf behaviors generated = %d, want %d", len(leaves), behaviorCount)
	}
	if !kinds[refAll] || !kinds[refAny] {
		t.Errorf("composite kinds generated = %v, want both All and Any", kinds)
	}
	for depth := 1; depth <= equivalenceDepth; depth++ {
		if !depths[depth] {
			t.Errorf("no tree of depth %d generated; depths = %v", depth, depths)
		}
	}
	// Structural variety is still not outcome variety, and outcomes are what
	// the differential test compares. If every seed expected 403, a mutation
	// that only ever turns a 500 into a 400 would sail through. Each of the four
	// statuses the middleware can produce needs a real share of the range.
	for _, status := range []int{
		http.StatusNoContent,
		http.StatusForbidden,
		http.StatusBadRequest,
		http.StatusInternalServerError,
	} {
		if statuses[status]*20 < equivalenceSeeds {
			t.Errorf("status %d expected by %d of %d seeds, want at least 5%%", status, statuses[status], equivalenceSeeds)
		}
	}
}

func TestReferenceMatchesEveryLeafBehavior(t *testing.T) {
	// The scripted behaviors are the alphabet the generator draws from, so a
	// leaf that quietly stopped doing what its name says would corrupt every
	// tree above it while the differential test stayed green — the reference
	// table and the Rego would be wrong in the same direction. This is the one
	// place each behavior is checked on its own.
	//
	// Every behavior in every source variant, because a variant is a different
	// module: the shared one scripts all six behaviors side by side and picks
	// between them by rule name, so a rule that named the wrong thing there
	// would make the fuzz target fail against leaves nobody implemented rather
	// than against a bug in the optimizer.
	for variant := sourceVariant(0); variant < sourceVariantCount; variant++ {
		for scripted := behavior(0); scripted < behaviorCount; scripted++ {
			assertMatchesReference(t, unoptimizedMode, fmt.Sprintf("behavior %d from source %d", scripted, variant),
				refTree{kind: refLeaf, leaf: scripted, source: variant})
		}
	}
}

func TestEvaluatorMatchesReferenceSemantics(t *testing.T) {
	for seed := int64(1); seed <= equivalenceSeeds; seed++ {
		tree := genTree(rand.New(rand.NewSource(seed)), equivalenceDepth)
		// Errorf inside assertMatchesReference, so a systematic disagreement
		// reports every seed it affects rather than only the lowest one.
		assertMatchesReference(t, unoptimizedMode, fmt.Sprintf("seed %d", seed), tree)
	}
}

// TestOptimizerPreservesSemantics is the point of everything above: the same
// comparison with the optimizer left on. It was written before any pass
// existed and parked as a comment until Optimize landed, because an oracle
// added alongside the thing it judges is an oracle fitted to it.
//
// It cannot tell a working optimizer from one that returns its input — a no-op
// preserves semantics perfectly. TestDefaultPipelineRewritesGeneratedTrees in
// policy_optimize_test.go is the other half of that pair.
func TestOptimizerPreservesSemantics(t *testing.T) {
	for seed := int64(1); seed <= equivalenceSeeds; seed++ {
		tree := genTree(rand.New(rand.NewSource(seed)), equivalenceDepth)
		assertMatchesReference(t, optimizedMode, fmt.Sprintf("seed %d", seed), tree)
	}
}
