package handlers

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"syscall"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/config"
	"cyclops-cs-backend/githubtrust"

	"github.com/jackc/pgx/v5/pgconn"
)

type fakeGitHubTrustStore struct {
	listContext      context.Context
	createContext    context.Context
	getContext       context.Context
	updateContext    context.Context
	deleteContext    context.Context
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
	resolveContext   context.Context
}

func (f *fakeGitHubTrustStore) List(ctx context.Context, ownerSub string) ([]*githubtrust.Policy, error) {
	f.listContext = ctx
	listErr := f.listErr
	if listErr != nil {
		return nil, listErr

	}
	out := make([]*githubtrust.Policy, len(f.listByOwner))
	copy(out, f.listByOwner)
	return out, nil
}

func (f *fakeGitHubTrustStore) Create(ctx context.Context, policy *githubtrust.Policy) error {
	f.createContext = ctx
	createErr := f.createErr
	if createErr != nil {
		return createErr

	}
	cp := *policy
	f.createInput = &cp
	return nil
}

func (f *fakeGitHubTrustStore) Get(ctx context.Context, ownerSub, id string) (*githubtrust.Policy, error) {
	f.getContext = ctx
	_ = ownerSub
	_ = id
	getErr := f.getErr
	if getErr != nil {
		return nil, getErr

	}
	if f.getResult == nil {
		return nil, nil
	}
	cp := *f.getResult
	return &cp, nil
}

func (f *fakeGitHubTrustStore) Update(ctx context.Context, policy *githubtrust.Policy) error {
	f.updateContext = ctx
	updateErr := f.updateErr
	if updateErr != nil {
		return updateErr

	}
	cp := *policy
	f.updateInput = &cp
	return nil
}

func (f *fakeGitHubTrustStore) Delete(ctx context.Context, ownerSub, id string) (bool, error) {
	f.deleteContext = ctx
	f.deleteOwner = ownerSub
	f.deleteID = id
	deleteErr := f.deleteErr
	if deleteErr != nil {
		return false, deleteErr

	}
	return f.deleteFound, nil
}

func (f *fakeGitHubTrustStore) ResolveByRepository(ctx context.Context, repository string) ([]*githubtrust.Policy, error) {
	f.resolveContext = ctx
	f.resolvedRepoName = repository
	resolveErr := f.resolveErr
	if resolveErr != nil {
		return nil, resolveErr

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

	h := Handlers{Features: FeaturesWith(nil, store), AuthCfg: authConfigForHandlers()}
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
	h := Handlers{Features: FeaturesWith(nil, store)}
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
	h := Handlers{Features: FeaturesWith(nil, store)}
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
	h := Handlers{Features: FeaturesWith(nil, store)}
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
	h := Handlers{Features: FeaturesWith(nil, store)}
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

func TestGitHubTrustPoliciesUseBoundedDatabaseContext(t *testing.T) {
	policy := &githubtrust.Policy{
		ID:                "policy-1",
		OwnerSub:          "user-123",
		Name:              "ci",
		Repository:        "trycua/cloud",
		AllowedNamespaces: []string{"ns-a"},
		Enabled:           true,
		CreatedAt:         time.Date(2026, 6, 26, 10, 0, 0, 0, time.UTC),
	}
	tests := []struct {
		name    string
		request *http.Request
		call    func(Handlers, http.ResponseWriter, *http.Request)
		context func(*fakeGitHubTrustStore) context.Context
	}{
		{"list", httptest.NewRequest(http.MethodGet, "/api/github-trust-policies", nil), Handlers.ListGitHubTrustPolicies, func(store *fakeGitHubTrustStore) context.Context { return store.listContext }},
		{"create", httptest.NewRequest(http.MethodPost, "/api/github-trust-policies", bytes.NewBufferString(`{"name":"ci","repository":"trycua/cloud","allowed_namespaces":["ns-a"],"enabled":true}`)), Handlers.CreateGitHubTrustPolicy, func(store *fakeGitHubTrustStore) context.Context { return store.createContext }},
		{"update", httptest.NewRequest(http.MethodPatch, "/api/github-trust-policies/policy-1", bytes.NewBufferString(`{"enabled":false}`)), Handlers.UpdateGitHubTrustPolicy, func(store *fakeGitHubTrustStore) context.Context { return store.updateContext }},
		{"delete", httptest.NewRequest(http.MethodDelete, "/api/github-trust-policies/policy-1", nil), Handlers.DeleteGitHubTrustPolicy, func(store *fakeGitHubTrustStore) context.Context { return store.deleteContext }},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			store := &fakeGitHubTrustStore{getResult: policy, deleteFound: true}
			request := withUser(test.request, &auth.User{ID: "user-123", AZP: "cyclops-cs-spa"})
			if test.name == "update" || test.name == "delete" {
				request.SetPathValue("id", "policy-1")
			}
			response := httptest.NewRecorder()
			start := time.Now()

			test.call(Handlers{Features: FeaturesWith(nil, store)}, response, request)

			assertBoundedDatabaseContext(t, test.context(store), start)
			if test.name == "update" {
				assertBoundedDatabaseContext(t, store.getContext, start)
			}
		})
	}
}

func assertBoundedDatabaseContext(t *testing.T, ctx context.Context, start time.Time) {
	t.Helper()
	if ctx == nil {
		t.Fatal("store context was not captured")
	}
	deadline, ok := ctx.Deadline()
	if !ok {
		t.Fatal("store context has no deadline")
	}
	if deadline.Before(start) {
		t.Fatalf("deadline = %s, before test start %s", deadline, start)
	}
	if deadline.After(start.Add(databaseRequestTimeout + time.Second)) {
		t.Fatalf("deadline = %s, want no later than %s", deadline, start.Add(databaseRequestTimeout+time.Second))
	}
	if !errors.Is(ctx.Err(), context.Canceled) {
		t.Fatalf("context error after handler returned = %v, want context canceled", ctx.Err())
	}
}

func TestGitHubTrustResolverUsesBoundedDatabaseContext(t *testing.T) {
	store := &fakeGitHubTrustStore{}
	resolver := NewGitHubTrustResolver(store)
	start := time.Now()

	if _, err := resolver.ResolveGitHubTrustPolicies(context.Background(), "trycua/cloud"); err != nil {
		t.Fatalf("resolve github trust policies: %v", err)
	}

	assertBoundedDatabaseContext(t, store.resolveContext, start)
}

func TestGitHubTrustResolverHonorsExistingDatabaseDeadline(t *testing.T) {
	store := &fakeGitHubTrustStore{}
	resolver := NewGitHubTrustResolver(store)
	parent, cancel := context.WithDeadline(context.Background(), time.Now().Add(time.Second))
	defer cancel()
	wantDeadline, ok := parent.Deadline()
	if !ok {
		t.Fatal("parent context has no deadline")
	}

	if _, err := resolver.ResolveGitHubTrustPolicies(parent, "trycua/cloud"); err != nil {
		t.Fatalf("resolve github trust policies: %v", err)
	}

	gotDeadline, ok := store.resolveContext.Deadline()
	if !ok {
		t.Fatal("resolver context has no deadline")
	}
	if !gotDeadline.Equal(wantDeadline) {
		t.Fatalf("resolver deadline = %s, want parent deadline %s", gotDeadline, wantDeadline)
	}
}

func TestGitHubTrustResolverNormalizesDatabaseFailures(t *testing.T) {
	store := &fakeGitHubTrustStore{resolveErr: context.DeadlineExceeded}
	resolver := NewGitHubTrustResolver(store)

	_, err := resolver.ResolveGitHubTrustPolicies(context.Background(), "trycua/cloud")
	if !errors.Is(err, auth.ErrDatabaseUnavailable) {
		t.Fatalf("resolve error = %v, want database unavailable sentinel", err)
	}
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("resolve error = %v, want original deadline error", err)
	}
}

func TestGitHubTrustResolverClassifiesImmediateNetworkFailures(t *testing.T) {
	connectionRefused := &net.OpError{Op: "dial", Net: "tcp", Err: syscall.ECONNREFUSED}
	store := &fakeGitHubTrustStore{resolveErr: connectionRefused}
	resolver := NewGitHubTrustResolver(store)

	_, err := resolver.ResolveGitHubTrustPolicies(context.Background(), "trycua/cloud")
	if !errors.Is(err, auth.ErrDatabaseUnavailable) {
		t.Fatalf("resolve error = %v, want database unavailable sentinel", err)
	}
	if !errors.Is(err, connectionRefused) {
		t.Fatalf("resolve error = %v, want original connection error", err)
	}
}

func TestGitHubTrustResolverDoesNotClassifyPostgresErrorsAsUnavailable(t *testing.T) {
	postgresError := &pgconn.PgError{Code: "42501", Message: "permission denied"}
	store := &fakeGitHubTrustStore{resolveErr: postgresError}
	resolver := NewGitHubTrustResolver(store)

	_, err := resolver.ResolveGitHubTrustPolicies(context.Background(), "trycua/cloud")
	if errors.Is(err, auth.ErrDatabaseUnavailable) {
		t.Fatalf("resolve error = %v, must not be database unavailable", err)
	}
}

func TestGitHubTrustPolicies_DatabaseFailuresReturnServiceUnavailable(t *testing.T) {
	tests := []struct {
		name string
		err  error
	}{
		{name: "unavailable", err: auth.ErrDatabaseUnavailable},
		{name: "deadline", err: context.DeadlineExceeded},
		{name: "canceled", err: context.Canceled},
		{name: "connection refused", err: &net.OpError{Op: "dial", Net: "tcp", Err: syscall.ECONNREFUSED}},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			store := &fakeGitHubTrustStore{listErr: test.err}
			h := Handlers{Features: FeaturesWith(nil, store)}
			request := withUser(httptest.NewRequest(http.MethodGet, "/api/github-trust-policies", nil), &auth.User{ID: "user-123"})
			response := httptest.NewRecorder()

			h.ListGitHubTrustPolicies(response, request)

			if response.Code != http.StatusServiceUnavailable {
				t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
			}
		})
	}
}

func TestGitHubTrustPolicies_PostgresErrorsRemainInternalServerErrors(t *testing.T) {
	store := &fakeGitHubTrustStore{listErr: &pgconn.PgError{Code: "42501", Message: "permission denied"}}
	h := Handlers{Features: FeaturesWith(nil, store)}
	request := withUser(httptest.NewRequest(http.MethodGet, "/api/github-trust-policies", nil), &auth.User{ID: "user-123"})
	response := httptest.NewRecorder()

	h.ListGitHubTrustPolicies(response, request)

	if response.Code != http.StatusInternalServerError {
		t.Fatalf("status = %d, want 500; body = %s", response.Code, response.Body.String())
	}
}

func TestUpdateGitHubTrustPolicySharesDatabaseBudget(t *testing.T) {
	policy := &githubtrust.Policy{
		ID: "policy-1", OwnerSub: "user-123", Name: "ci", Repository: "trycua/cloud",
		AllowedNamespaces: []string{"ns-a"}, Enabled: true,
	}
	store := &fakeGitHubTrustStore{getResult: policy}
	h := Handlers{Features: FeaturesWith(nil, store)}
	request := withUser(httptest.NewRequest(http.MethodPatch, "/api/github-trust-policies/policy-1", bytes.NewBufferString(`{"enabled":false}`)), &auth.User{ID: "user-123"})
	request.SetPathValue("id", "policy-1")
	response := httptest.NewRecorder()

	h.UpdateGitHubTrustPolicy(response, request)

	if response.Code != http.StatusOK {
		t.Fatalf("status = %d, body = %s", response.Code, response.Body.String())
	}
	getDeadline, getOK := store.getContext.Deadline()
	updateDeadline, updateOK := store.updateContext.Deadline()
	if !getOK || !updateOK {
		t.Fatalf("deadlines get=%t update=%t, want both", getOK, updateOK)
	}
	if !getDeadline.Equal(updateDeadline) {
		t.Fatalf("deadlines get=%s update=%s, want one shared budget", getDeadline, updateDeadline)
	}
}
