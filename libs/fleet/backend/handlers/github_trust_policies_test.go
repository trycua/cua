package handlers

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/config"
	"cyclops-cs-backend/githubtrust"
)

type fakeGitHubTrustStore struct {
	listByOwner      []*githubtrust.Policy
	createInput      *githubtrust.Policy
	updateInput      *githubtrust.Policy
	deleteOwner      string
	deleteID         string
	deleteFound      bool
	deleteErr        error
	getResult        *githubtrust.Policy
	getErr           error
	listErr          error
	createErr        error
	updateErr        error
	resolveResult    []*githubtrust.Policy
	resolveErr       error
	resolvedRepoName string
}

func (f *fakeGitHubTrustStore) List(ctx context.Context, ownerSub string) ([]*githubtrust.Policy, error) {
	_ = ctx
	if f.listErr != nil {
		return nil, f.listErr
	}
	out := make([]*githubtrust.Policy, len(f.listByOwner))
	copy(out, f.listByOwner)
	return out, nil
}

func (f *fakeGitHubTrustStore) Create(ctx context.Context, policy *githubtrust.Policy) error {
	_ = ctx
	if f.createErr != nil {
		return f.createErr
	}
	cp := *policy
	f.createInput = &cp
	return nil
}

func (f *fakeGitHubTrustStore) Get(ctx context.Context, ownerSub, id string) (*githubtrust.Policy, error) {
	_ = ctx
	_ = ownerSub
	_ = id
	if f.getErr != nil {
		return nil, f.getErr
	}
	if f.getResult == nil {
		return nil, nil
	}
	cp := *f.getResult
	return &cp, nil
}

func (f *fakeGitHubTrustStore) Update(ctx context.Context, policy *githubtrust.Policy) error {
	_ = ctx
	if f.updateErr != nil {
		return f.updateErr
	}
	cp := *policy
	f.updateInput = &cp
	return nil
}

func (f *fakeGitHubTrustStore) Delete(ctx context.Context, ownerSub, id string) (bool, error) {
	_ = ctx
	f.deleteOwner = ownerSub
	f.deleteID = id
	if f.deleteErr != nil {
		return false, f.deleteErr
	}
	return f.deleteFound, nil
}

func (f *fakeGitHubTrustStore) ResolveByRepository(ctx context.Context, repository string) ([]*githubtrust.Policy, error) {
	_ = ctx
	f.resolvedRepoName = repository
	if f.resolveErr != nil {
		return nil, f.resolveErr
	}
	out := make([]*githubtrust.Policy, len(f.resolveResult))
	copy(out, f.resolveResult)
	return out, nil
}

func TestListGitHubTrustPolicies_Success(t *testing.T) {
	store := &fakeGitHubTrustStore{
		listByOwner: []*githubtrust.Policy{{
			ID:                "policy-1",
			OwnerSub:          "user-123",
			Name:              "repo access",
			Repository:        "trycua/cloud",
			AllowedNamespaces: []string{"ns-a", "ns-b"},
			Enabled:           true,
			CreatedAt:         time.Date(2026, 6, 26, 10, 0, 0, 0, time.UTC),
			UpdatedAt:         time.Date(2026, 6, 26, 11, 0, 0, 0, time.UTC),
		}},
	}

	h := Handlers{GitHubTrustPolicies: store, AuthCfg: authConfigForHandlers()}
	r := httptest.NewRequest(http.MethodGet, "/api/github-trust-policies", nil)
	r = withUser(r, &auth.User{ID: "user-123", AZP: "cyclops-cs-spa"})
	w := httptest.NewRecorder()

	h.ListGitHubTrustPolicies(w, r)

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
	}

	var resp GitHubTrustPolicyListResponse
	if err := json.NewDecoder(w.Body).Decode(&resp); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	if resp.OIDC.Issuer != "https://token.actions.githubusercontent.com" {
		t.Fatalf("issuer = %q", resp.OIDC.Issuer)
	}
	if resp.OIDC.Audience != "fleets" {
		t.Fatalf("audience = %q, want fleets", resp.OIDC.Audience)
	}
	if len(resp.Policies) != 1 || resp.Policies[0].Repository != "trycua/cloud" {
		t.Fatalf("unexpected policies payload: %+v", resp.Policies)
	}
}

func TestCreateGitHubTrustPolicy_ValidatesAndPersists(t *testing.T) {
	store := &fakeGitHubTrustStore{}
	h := Handlers{GitHubTrustPolicies: store}
	body := `{"name":"ci","repository":"trycua/cloud","allowed_namespaces":["ns-b","ns-a","ns-a"],"enabled":true}`
	r := httptest.NewRequest(http.MethodPost, "/api/github-trust-policies", bytes.NewBufferString(body))
	r = withUser(r, &auth.User{ID: "user-123", AZP: "cyclops-cs-spa"})
	w := httptest.NewRecorder()

	h.CreateGitHubTrustPolicy(w, r)

	if w.Code != http.StatusCreated {
		t.Fatalf("status = %d, want 201; body = %s", w.Code, w.Body.String())
	}
	if store.createInput == nil {
		t.Fatal("expected store.Create to be called")
	}
	if store.createInput.OwnerSub != "user-123" {
		t.Fatalf("owner_sub = %q, want user-123", store.createInput.OwnerSub)
	}
	if got, want := store.createInput.AllowedNamespaces, []string{"ns-a", "ns-b"}; len(got) != len(want) || got[0] != want[0] || got[1] != want[1] {
		t.Fatalf("allowed_namespaces = %#v, want %#v", got, want)
	}
}

func TestPatchGitHubTrustPolicy_NotFound(t *testing.T) {
	store := &fakeGitHubTrustStore{}
	h := Handlers{GitHubTrustPolicies: store}
	r := httptest.NewRequest(http.MethodPatch, "/api/github-trust-policies/missing", bytes.NewBufferString(`{"enabled":false}`))
	r.SetPathValue("id", "missing")
	r = withUser(r, &auth.User{ID: "user-123", AZP: "cyclops-cs-spa"})
	w := httptest.NewRecorder()

	h.UpdateGitHubTrustPolicy(w, r)

	if w.Code != http.StatusNotFound {
		t.Fatalf("status = %d, want 404; body = %s", w.Code, w.Body.String())
	}
}

func TestDeleteGitHubTrustPolicy_Success(t *testing.T) {
	store := &fakeGitHubTrustStore{deleteFound: true}
	h := Handlers{GitHubTrustPolicies: store}
	r := httptest.NewRequest(http.MethodDelete, "/api/github-trust-policies/policy-1", nil)
	r.SetPathValue("id", "policy-1")
	r = withUser(r, &auth.User{ID: "user-123", AZP: "cyclops-cs-spa"})
	w := httptest.NewRecorder()

	h.DeleteGitHubTrustPolicy(w, r)

	if w.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want 204; body = %s", w.Code, w.Body.String())
	}
	if store.deleteOwner != "user-123" || store.deleteID != "policy-1" {
		t.Fatalf("delete args = %q/%q", store.deleteOwner, store.deleteID)
	}
}

func TestGitHubTrustPolicies_DisabledStore(t *testing.T) {
	h := Handlers{}
	r := httptest.NewRequest(http.MethodGet, "/api/github-trust-policies", nil)
	r = withUser(r, &auth.User{ID: "user-123", AZP: "cyclops-cs-spa"})
	w := httptest.NewRecorder()

	h.ListGitHubTrustPolicies(w, r)

	if w.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, want 503; body = %s", w.Code, w.Body.String())
	}
}

func TestGitHubTrustPolicies_StoreError(t *testing.T) {
	store := &fakeGitHubTrustStore{listErr: errors.New("redis down")}
	h := Handlers{GitHubTrustPolicies: store}
	r := httptest.NewRequest(http.MethodGet, "/api/github-trust-policies", nil)
	r = withUser(r, &auth.User{ID: "user-123", AZP: "cyclops-cs-spa"})
	w := httptest.NewRecorder()

	h.ListGitHubTrustPolicies(w, r)

	if w.Code != http.StatusInternalServerError {
		t.Fatalf("status = %d, want 500; body = %s", w.Code, w.Body.String())
	}
}

func authConfigForHandlers() config.AuthConfiguration {
	return config.AuthConfiguration{
		GitHubOIDCIssuer:          "https://token.actions.githubusercontent.com",
		GitHubOIDCAudience:        "fleets",
		GitHubOIDCLegacyAudiences: []string{"cyclops-cs"},
	}
}
