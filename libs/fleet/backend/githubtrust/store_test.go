package githubtrust

import (
	"testing"
	"time"
)

func TestNormalizePolicyInput(t *testing.T) {
	policy, err := NormalizePolicyInput(PolicyInput{
		Name:              "ci access",
		Repository:        "trycua/cloud",
		AllowedNamespaces: []string{"ns-b", "ns-a", "ns-a"},
		Enabled:           true,
	})
	if err != nil {
		t.Fatalf("NormalizePolicyInput err = %v", err)
	}
	if policy.Repository != "trycua/cloud" {
		t.Fatalf("repository = %q", policy.Repository)
	}
	if got, want := policy.AllowedNamespaces, []string{"ns-a", "ns-b"}; len(got) != len(want) || got[0] != want[0] || got[1] != want[1] {
		t.Fatalf("allowed_namespaces = %#v, want %#v", got, want)
	}
}

func TestNormalizePolicyInput_RejectsInvalidRepository(t *testing.T) {
	if _, err := NormalizePolicyInput(PolicyInput{
		Name:              "bad repo",
		Repository:        "not a repo",
		AllowedNamespaces: []string{"ns-a"},
		Enabled:           true,
	}); err == nil {
		t.Fatal("expected invalid repository to fail")
	}
}

func TestPolicySortNewestFirst(t *testing.T) {
	policies := []*Policy{
		{ID: "old", UpdatedAt: time.Date(2026, 6, 1, 0, 0, 0, 0, time.UTC)},
		{ID: "new", UpdatedAt: time.Date(2026, 6, 3, 0, 0, 0, 0, time.UTC)},
		{ID: "mid", UpdatedAt: time.Date(2026, 6, 2, 0, 0, 0, 0, time.UTC)},
	}
	sortByUpdatedDesc(policies)
	if policies[0].ID != "new" || policies[1].ID != "mid" || policies[2].ID != "old" {
		t.Fatalf("order = %s,%s,%s", policies[0].ID, policies[1].ID, policies[2].ID)
	}
}
