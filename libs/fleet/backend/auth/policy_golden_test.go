package auth

import (
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// This file pins the optimized shape of the policy trees that real routes are
// built from. The optimizer is free to reorder authorization checks — that is
// its whole purpose, and the lattice in policy_plan.go is why it is allowed to
// — but "free to reorder" must not mean "reorders without anyone noticing".
//
// The plans below are read out of the surface table in policy_routes.go, which
// is the same table main.go's route builders resolve through. There is no copy
// to drift: reorder a tree's children and this file diffs, because the snapshot
// holds the production value rather than a likeness. What it pins is the
// optimizer's output for those trees, so a changed pass or cost model diffs too.
//
// The pinning stops at PolicyMiddleware's door, though: which surface a route
// is bound to, and with which options, is not covered here. A route moved to a
// different surface, or a surface that starts passing WithPipeline, changes what
// that route runs without any plan here changing — TestPolicyAndRouterAgreeOnRoutes
// and the characterization table are what watch that axis.
//
// What is not pinned at all, so nobody mistakes this for more than it is:
//
//   - Policy meaning. A source renders as its key, and Registered("authz")
//     is the literal string "registered:authz". Rewrite authz.rego to allow
//     everything and every test in this file still passes. Rego semantics are
//     policy_test.go's job.
//   - Everything below the AST. Compile and fold are unrendered, so a change
//     to the body budget or to how the fold treats errors leaves the snapshot
//     byte-identical.
//   - Everything above it. Whether PolicyMiddleware still routes through
//     DefaultPipeline() at all is invisible here, because these tests call
//     Optimize directly with DefaultPipeline themselves.
//   - Which file an fs-backed leaf reads. Explain renders those as their Go
//     type and drops the path, so two FromFS leaves on different paths share
//     one rendering.
//   - Fact-provider behaviour behind an equal CacheKey.
//
// Note also that the optimizer is still a no-op on every plan here, measured
// across all ten — so this file gives no coverage of the optimizer
// whatsoever. Run it with an empty pipeline and it still passes.
//
// That survived the arrival of the first fact loader, which is worth stating
// rather than assuming. svc and namespaces carry a leaf that calls a
// FactProvider — precisely what sinkFactLoaders exists to move — and it moves
// nothing, because NamespaceOwnershipPolicy already declares its cheap children
// first and the sort is stable. The pass is load-bearing in the sense that
// matters (it is what keeps that order true if the declaration changes) and
// invisible in the sense this file measures. What the order buys is counted as
// probes in policy_ownership_test.go, not read off a shape here. Until a pinned
// plan exists that the optimizer actually rewrites, this is production-order
// documentation with a regeneration workflow, and the optimizer's own coverage
// lives in policy_equivalence_test.go and policy_fuzz_test.go.
//
// The snapshots are one file per plan, named by the surface. One combined file
// would also work, but a failure then prints every plan when one changed;
// per-plan files keep the failure message down to the plan that actually moved
// and keep each file's git history about one surface. Adding a surface needs no
// entry here at all — only one -update-golden run to write its snapshot.
//
// Explain does not sort a leaf's Facts, though key() does. Fact order carries
// no meaning — forPolicy builds a map — so a pass that permutes Facts produces
// a golden diff with no semantic change behind it.

var updateGolden = flag.Bool("update-golden", false, "rewrite golden plan files")

// goldenPlans is every policy tree whose optimized shape is pinned: one per
// authorization surface. Each entry owns testdata/<name>-plan.txt.
//
// The list is derived from the surface table in policy_routes.go rather than
// written out here, so a surface added there is pinned from the moment it exists
// — the failure is a missing snapshot file, which -update-golden fixes, rather
// than a plan nobody pinned. The trees are rebuilt per call rather than stored,
// so that a pass which mutated its input could not leak that mutation from one
// test into the next.
func goldenPlans(t *testing.T) []struct {
	name string
	tree func() Node
} {
	t.Helper()
	names := SurfaceNames()
	plans := make([]struct {
		name string
		tree func() Node
	}, 0, len(names))
	for _, name := range names {
		if _, ok := SurfaceTree(name); !ok {
			t.Fatalf("SurfaceNames returned %q, which SurfaceTree does not resolve", name)
		}
		plans = append(plans, struct {
			name string
			tree func() Node
		}{
			name: name + "-route",
			tree: func() Node {
				tree, _ := SurfaceTree(name)
				return tree
			},
		})
	}
	return plans
}

func goldenPlanPath(name string) string {
	return filepath.Join("testdata", name+"-plan.txt")
}

// TestGoldenPlansAreStable is the tripwire: it fails when the optimized plan
// for a real route stops looking exactly like the checked-in snapshot, whether
// because the tree here drifted from main.go, because a pass changed, or
// because the cost model started ordering leaves differently.
func TestGoldenPlansAreStable(t *testing.T) {
	for _, plan := range goldenPlans(t) {
		t.Run(plan.name, func(t *testing.T) {
			got := Explain(Optimize(plan.tree(), DefaultPipeline()))
			path := goldenPlanPath(plan.name)

			if *updateGolden {
				if err := os.MkdirAll("testdata", 0o755); err != nil {
					t.Fatalf("mkdir testdata: %v", err)
				}
				if err := os.WriteFile(path, []byte(got), 0o644); err != nil {
					t.Fatalf("write %s: %v", path, err)
				}
				return
			}

			want, err := os.ReadFile(path)
			if err != nil {
				t.Fatalf("read %s (regenerate with: go test ./auth/ -run TestGoldenPlansAreStable -update-golden): %v", path, err)
			}
			if got != string(want) {
				t.Fatalf(`the optimized %q plan changed.

--- want (%s) ---
%s--- got ---
%s
This is the order authorization checks run in on a production route. The
lattice guarantees the verdict is the same either way, so do not go hunting
for an allow that became a deny — what an order change moves is which leaves
run at all, and so which fact loads and body reads happen, and which errors
fold discards once a sibling has decided. Read the two trees above and satisfy
yourself the new order is what you meant, then regenerate the snapshot with:

    go test ./auth/ -run TestGoldenPlansAreStable -update-golden`,
					plan.name, path, want, got)
			}
		})
	}
}

// TestGoldenPlansCompile checks that the pinned trees are not just stable but
// buildable: Compile is what loads each source's modules and prepares the Rego,
// so a snapshot naming a module nobody registered would otherwise stay green.
//
// LoadOpa is what registers "authz" and "pool-admission". There is no init in
// this package that does it, so the test has to call it — and calling the real
// registrar rather than re-registering the modules here is the point: a
// production module renamed in LoadOpa fails this test instead of passing
// against a private copy.
func TestGoldenPlansCompile(t *testing.T) {
	LoadOpa()
	for _, plan := range goldenPlans(t) {
		t.Run(plan.name, func(t *testing.T) {
			if _, err := Compile(Optimize(plan.tree(), DefaultPipeline())); err != nil {
				t.Fatalf("compile %q plan: %v", plan.name, err)
			}
		})
	}
}

// TestGoldenPlanFilesHaveNoOrphans catches the other direction of drift: a plan
// renamed or deleted from the table leaves its old snapshot behind, and a stale
// snapshot is worse than none — it looks like a pinned plan that no test reads.
func TestGoldenPlanFilesHaveNoOrphans(t *testing.T) {
	plans := goldenPlans(t)
	expected := make(map[string]bool, len(plans))
	for _, plan := range plans {
		expected[goldenPlanPath(plan.name)] = true
	}
	// Scoped to *-plan.txt so unrelated fixtures may share testdata/.
	found, err := filepath.Glob(filepath.Join("testdata", "*-plan.txt"))
	if err != nil {
		t.Fatalf("glob testdata: %v", err)
	}
	for _, path := range found {
		if !expected[path] {
			t.Errorf("%s is not produced by any entry in goldenPlans; delete it or add the plan back", path)
		}
	}
}

// TestGoldenPlanNamesAreUniqueAndPathSafe guards the axis the orphan check
// cannot see. Two entries sharing a name both write one file, and under
// -update-golden the second silently overwrites the first — so the pinned plan
// is lost before the collision is noticed, and a confusing golden failure is
// exactly when someone reaches for -update-golden. A name carrying a path
// separator fails the write instead with a bare ENOENT, because the snapshot
// writer creates testdata/ but not subdirectories under it.
func TestGoldenPlanNamesAreUniqueAndPathSafe(t *testing.T) {
	plans := goldenPlans(t)
	seen := make(map[string]bool, len(plans))
	for _, plan := range plans {
		if plan.name == "" || strings.ContainsAny(plan.name, `/\`) {
			t.Errorf("plan name %q is empty or contains a path separator", plan.name)
		}
		if seen[plan.name] {
			t.Errorf("duplicate plan name %q; each entry owns one testdata file", plan.name)
		}
		seen[plan.name] = true
	}
}
