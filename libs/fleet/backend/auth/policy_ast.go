package auth

import (
	"fmt"
	"slices"
	"strings"
)

// Node is the policy AST. It is pure data: no compiled Rego, no fs.FS
// handles, no I/O. Optimizers rewrite Node trees; Compile turns a Node into
// an executable plan.
type Node interface{ node() }

// PolicyExpression is the historical name for Node, kept so existing call
// sites and tests continue to compile.
type PolicyExpression = Node

// Leaf is a single Rego query over one or more modules.
type Leaf struct {
	Source  PolicySource
	Query   string
	Facts   []factProviderConfig
	MaxBody int64
}

func (Leaf) node() {}

// AllNode is conjunction.
type AllNode struct{ Children []Node }

func (AllNode) node() {}

// AnyNode is disjunction.
type AnyNode struct{ Children []Node }

func (AnyNode) node() {}

// BecauseNode associates one public denial reason with a policy subtree.
type BecauseNode struct {
	Child  Node
	Reason string
}

func (BecauseNode) node() {}

func All(children ...Node) Node {
	if len(children) == 0 {
		panic("OPA All requires at least one policy")
	}
	return AllNode{Children: children}
}

func Any(children ...Node) Node {
	if len(children) == 0 {
		panic("OPA Any requires at least one policy")
	}
	return AnyNode{Children: children}
}

func Because(child Node, reason string) Node {
	if child == nil {
		panic("OPA Because requires a policy")
	}
	if reason == "" {
		panic("OPA Because requires a denial reason")
	}
	return BecauseNode{Child: child, Reason: reason}
}

// Policy builds a leaf. It performs no I/O and no compilation; both happen in
// Compile. It still panics on a missing query because that is a pure-data
// programming error detectable without touching the filesystem.
func Policy(source PolicySource, options ...PolicyOption) Node {
	config := policyConfig{}
	for _, option := range options {
		option(&config)
	}
	if config.query == "" {
		panic("OPA policy requires a query")
	}
	if source == nil {
		panic("OPA policy requires a source")
	}
	return Leaf{
		Source:  source,
		Query:   config.query,
		Facts:   config.factProviders,
		MaxBody: config.maxBodyBytes,
	}
}

// key is the structural identity Dedupe uses to decide whether two leaves may
// be merged. It returns "" when the leaf has no stable identity, and any caller
// comparing keys must treat "" as never equal to anything, including itself.
//
// Every component is quoted before joining, for the same reason
// multiPolicySource.sourceKey quotes its children: without it the separators
// are ambiguous and two different leaves can encode to one key. That is not
// theoretical here — a source name and a query are both arbitrary strings, so
// Policy(Registered("x"), Query("y|z")) and Policy(Registered("x|y"),
// Query("z")) collide under an unquoted encoding, and a collision means Dedupe
// deletes one of two policies that check different things.
func (leaf Leaf) key() string {
	if leaf.Source == nil {
		return ""
	}
	sourceKey := leaf.Source.sourceKey()
	if sourceKey == "" {
		return ""
	}
	facts := make([]string, 0, len(leaf.Facts))
	for _, configured := range leaf.Facts {
		facts = append(facts, fmt.Sprintf("%q=%q", configured.namespace, configured.provider.CacheKey()))
	}
	// Sorted so that two leaves listing the same providers in different orders
	// are one key: forPolicy builds a map from them, so order is not meaning.
	slices.Sort(facts)
	return fmt.Sprintf("leaf|%q|%q|%q|%d", sourceKey, leaf.Query, strings.Join(facts, ","), leaf.MaxBody)
}

// Explain renders a Node as an indented tree, for debugging, for golden-plan
// snapshots, and for the optimizer's fixpoint detection.
//
// A leaf whose source has no stable identity prints its Go type rather than
// its path, so two distinct fs sources can render identically. Fixpoint
// detection tolerates a false "equal" (it stops optimizing early, which is
// safe); deciding whether two leaves may be merged must not, which is why
// that decision belongs to key() instead.
func Explain(n Node) string {
	var builder strings.Builder
	explainInto(&builder, n, 0)
	return builder.String()
}

func explainInto(builder *strings.Builder, n Node, depth int) {
	indent := strings.Repeat("  ", depth)
	switch node := n.(type) {
	case Leaf:
		source := "<nil>"
		if node.Source != nil {
			if source = node.Source.sourceKey(); source == "" {
				source = fmt.Sprintf("%T", node.Source)
			}
		}
		fmt.Fprintf(builder, "%sLeaf query=%s source=%s", indent, node.Query, source)
		for _, configured := range node.Facts {
			fmt.Fprintf(builder, " facts=%s@%s", configured.namespace, configured.provider.CacheKey())
		}
		if node.MaxBody > 0 {
			fmt.Fprintf(builder, " body=%d", node.MaxBody)
		}
		builder.WriteString("\n")
	case AllNode:
		fmt.Fprintf(builder, "%sAll\n", indent)
		for _, child := range node.Children {
			explainInto(builder, child, depth+1)
		}
	case AnyNode:
		fmt.Fprintf(builder, "%sAny\n", indent)
		for _, child := range node.Children {
			explainInto(builder, child, depth+1)
		}
	case BecauseNode:
		fmt.Fprintf(builder, "%sBecause reason=%q\n", indent, node.Reason)
		explainInto(builder, node.Child, depth+1)
	default:
		fmt.Fprintf(builder, "%s<unknown %T>\n", indent, n)
	}
}
