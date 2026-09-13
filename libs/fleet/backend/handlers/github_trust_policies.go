package handlers

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/githubtrust"
)

type GitHubTrustPolicyResponse struct {
	ID                string    `json:"id"`
	OwnerSub          string    `json:"owner_sub"`
	Name              string    `json:"name"`
	Repository        string    `json:"repository"`
	AllowedNamespaces []string  `json:"allowed_namespaces"`
	Enabled           bool      `json:"enabled"`
	CreatedAt         time.Time `json:"created_at"`
	UpdatedAt         time.Time `json:"updated_at"`
}

type GitHubTrustPolicyListResponse struct {
	Policies []GitHubTrustPolicyResponse `json:"policies"`
	OIDC     struct {
		Issuer   string `json:"issuer"`
		Audience string `json:"audience"`
	} `json:"oidc"`
}

type GitHubTrustPolicyPatchRequest struct {
	Name              *string   `json:"name"`
	Repository        *string   `json:"repository"`
	AllowedNamespaces *[]string `json:"allowed_namespaces"`
	Enabled           *bool     `json:"enabled"`
}

func (h Handlers) ListGitHubTrustPolicies(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	if h.Features.TrustStore() == nil {
		writeErr(w, http.StatusServiceUnavailable, "github trust policies are not configured")
		return
	}
	ctx, cancel := databaseContext(r.Context())
	defer cancel()
	policies, err := h.Features.TrustStore().List(ctx, user.ID)
	if err != nil {
		writeGitHubTrustStoreErr(w, err, "failed to list github trust policies")
		return
	}
	resp := GitHubTrustPolicyListResponse{
		Policies: make([]GitHubTrustPolicyResponse, 0, len(policies)),
	}
	resp.OIDC.Issuer = h.AuthCfg.GitHubOIDCIssuer
	resp.OIDC.Audience = h.AuthCfg.GitHubOIDCAudience
	for _, policy := range policies {
		resp.Policies = append(resp.Policies, policyResponse(policy))
	}
	writeJSON(w, http.StatusOK, resp)
}

func (h Handlers) CreateGitHubTrustPolicy(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	if h.Features.TrustStore() == nil {
		writeErr(w, http.StatusServiceUnavailable, "github trust policies are not configured")
		return
	}
	var input githubtrust.PolicyInput
	if err := json.NewDecoder(r.Body).Decode(&input); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid body")
		return
	}
	policy, err := githubtrust.NormalizePolicyInput(input)
	if err != nil {
		writeTrustPolicyValidationErr(w, err)
		return
	}
	policy.OwnerSub = user.ID
	ctx, cancel := databaseContext(r.Context())
	defer cancel()
	if err := h.Features.TrustStore().Create(ctx, policy); err != nil {
		writeGitHubTrustStoreErr(w, err, "failed to create github trust policy")
		return
	}
	writeJSON(w, http.StatusCreated, policyResponse(policy))
}

func (h Handlers) UpdateGitHubTrustPolicy(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	if h.Features.TrustStore() == nil {
		writeErr(w, http.StatusServiceUnavailable, "github trust policies are not configured")
		return
	}
	id := r.PathValue("id")
	ctx, cancel := databaseContext(r.Context())
	defer cancel()
	current, err := h.Features.TrustStore().Get(ctx, user.ID, id)
	if err != nil {
		writeGitHubTrustStoreErr(w, err, "failed to load github trust policy")
		return
	}
	if current == nil {
		writeErr(w, http.StatusNotFound, "github trust policy not found")
		return
	}
	var patch GitHubTrustPolicyPatchRequest
	if err := json.NewDecoder(r.Body).Decode(&patch); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid body")
		return
	}
	input := githubtrust.PolicyInput{
		Name:              current.Name,
		Repository:        current.Repository,
		AllowedNamespaces: current.AllowedNamespaces,
		Enabled:           current.Enabled,
	}
	if patch.Name != nil {
		input.Name = *patch.Name
	}
	if patch.Repository != nil {
		input.Repository = *patch.Repository
	}
	if patch.AllowedNamespaces != nil {
		input.AllowedNamespaces = *patch.AllowedNamespaces
	}
	if patch.Enabled != nil {
		input.Enabled = *patch.Enabled
	}
	policy, err := githubtrust.NormalizePolicyInput(input)
	if err != nil {
		writeTrustPolicyValidationErr(w, err)
		return
	}
	policy.ID = current.ID
	policy.OwnerSub = current.OwnerSub
	policy.CreatedAt = current.CreatedAt
	if err := h.Features.TrustStore().Update(ctx, policy); err != nil {
		if errors.Is(err, githubtrust.ErrNotFound) {
			writeErr(w, http.StatusNotFound, "github trust policy not found")
			return
		}
		writeGitHubTrustStoreErr(w, err, "failed to update github trust policy")
		return
	}
	writeJSON(w, http.StatusOK, policyResponse(policy))
}

func (h Handlers) DeleteGitHubTrustPolicy(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	if h.Features.TrustStore() == nil {
		writeErr(w, http.StatusServiceUnavailable, "github trust policies are not configured")
		return
	}
	ctx, cancel := databaseContext(r.Context())
	defer cancel()
	found, err := h.Features.TrustStore().Delete(ctx, user.ID, r.PathValue("id"))
	if err != nil {
		writeGitHubTrustStoreErr(w, err, "failed to delete github trust policy")
		return
	}
	if !found {
		writeErr(w, http.StatusNotFound, "github trust policy not found")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// NewGitHubTrustResolver binds a resolver to one store. Production wiring uses
// NewGitHubTrustResolverFor instead, so the store can be installed after
// startup; this remains for tests that exercise the resolver against a fixed
// store.
func NewGitHubTrustResolver(store githubtrust.Store) auth.GitHubTrustResolver {
	if store == nil {
		return nil
	}
	return githubTrustResolver{store: store}
}

type githubTrustResolver struct {
	store githubtrust.Store
}

func (r githubTrustResolver) ResolveGitHubTrustPolicies(ctx context.Context, repository string) ([]auth.GitHubTrustPolicy, error) {
	databaseCtx, cancel := databaseContext(ctx)
	defer cancel()
	policies, err := r.store.ResolveByRepository(databaseCtx, repository)
	if err != nil {
		originErr := err
		err = auth.ClassifyDatabaseError(err)
		if auth.IsDatabaseUnavailable(err) {
			return nil, errors.Join(auth.DatabaseUnavailable(err), originErr)

		}
		return nil, errors.Join(err, originErr)

	}
	out := make([]auth.GitHubTrustPolicy, 0, len(policies))
	for _, policy := range policies {
		out = append(out, auth.GitHubTrustPolicy{
			ID:                policy.ID,
			OwnerSub:          policy.OwnerSub,
			Repository:        policy.Repository,
			AllowedNamespaces: policy.AllowedNamespaces,
			Enabled:           policy.Enabled,
		})
	}
	return out, nil
}

func policyResponse(policy *githubtrust.Policy) GitHubTrustPolicyResponse {
	return GitHubTrustPolicyResponse{
		ID:                policy.ID,
		OwnerSub:          policy.OwnerSub,
		Name:              policy.Name,
		Repository:        policy.Repository,
		AllowedNamespaces: policy.AllowedNamespaces,
		Enabled:           policy.Enabled,
		CreatedAt:         policy.CreatedAt,
		UpdatedAt:         policy.UpdatedAt,
	}
}

func writeGitHubTrustStoreErr(w http.ResponseWriter, err error, fallback string) {
	err = auth.ClassifyDatabaseError(err)
	if auth.IsDatabaseUnavailable(err) {
		writeErr(w, http.StatusServiceUnavailable, "github trust policies unavailable")
		return
	}
	writeErr(w, http.StatusInternalServerError, fallback)
}

func writeTrustPolicyValidationErr(w http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, githubtrust.ErrInvalidRepository),
		errors.Is(err, githubtrust.ErrEmptyNamespaces),
		errors.Is(err, githubtrust.ErrInvalidNamespace):
		writeErr(w, http.StatusBadRequest, err.Error())
	default:
		writeErr(w, http.StatusBadRequest, "invalid github trust policy")
	}
}
