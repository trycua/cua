package handlers

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"cyclops-cs-backend/accountlookup"
	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/keycloak"
)

type lookupTestIndex struct {
	outcomes []string
	auditErr error
}

func (*lookupTestIndex) Record(context.Context, string, string, string, string) error { return nil }
func (*lookupTestIndex) Resolve(context.Context, string, string, string) (string, bool, error) {
	return "", false, nil
}
func (*lookupTestIndex) Complete(context.Context, string, string) (bool, error) { return false, nil }
func (*lookupTestIndex) Allow(context.Context, string) (bool, error)            { return true, nil }
func (s *lookupTestIndex) Audit(_ context.Context, actor, outcome string) error {
	s.outcomes = append(s.outcomes, actor+":"+outcome)
	return s.auditErr
}

type lookupTestDirectory struct{}

func (lookupTestDirectory) LookupAccount(_ context.Context, id string) (*keycloak.Account, error) {
	return &keycloak.Account{ID: id, Email: "fixture@example.com"}, nil
}

func TestAccountLookupStrictJSONAndDenialAudit(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	index := &lookupTestIndex{}
	s := accountlookup.New(ctx, index, lookupTestDirectory{}, "fixture", "fixture-key", nil)
	defer func() { cancel(); s.Wait() }()
	admin := true
	h := Handlers{AccountLookup: s, adminAccessEvaluator: func(context.Context, *auth.User) (bool, error) { return admin, nil }}
	handler := PrivateAccountLookup(h.AuditAccountLookupAccess(http.HandlerFunc(h.LookupAccount)))
	for _, tc := range []struct {
		body string
		want int
	}{
		{`{"kind":"account_id","value":"fixture"}`, 200},
		{`{"kind":"account_id","value":"fixture","extra":"private-target"}`, 400},
		{`{"kind":"account_id","value":"fixture"} {}`, 400},
		{`{"kind":"account_id","value":"` + strings.Repeat("x", 1100) + `"}`, 400},
	} {
		r := httptest.NewRequest("POST", "/api/admin/account-lookup", strings.NewReader(tc.body))
		r = r.WithContext(context.WithValue(r.Context(), auth.UserKey, &auth.User{ID: "admin", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}))
		w := httptest.NewRecorder()
		handler.ServeHTTP(w, r)
		if w.Code != tc.want || w.Header().Get("Cache-Control") != "no-store" {
			t.Fatalf("response=%d %s", w.Code, w.Body.String())
		}
		if tc.want == 200 && !strings.Contains(w.Body.String(), "fixture@example.com") {
			t.Fatal("account not returned")
		}
		if tc.want != 200 && strings.Contains(w.Body.String(), "fixture") {
			t.Fatal("target leaked in error")
		}
	}
	admin = false
	r := httptest.NewRequest("POST", "/api/admin/account-lookup", strings.NewReader("private-target"))
	r = r.WithContext(context.WithValue(r.Context(), auth.UserKey, &auth.User{ID: "admin", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}))
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, r)
	if w.Code != 403 || index.outcomes[len(index.outcomes)-1] != "admin:forbidden" {
		t.Fatal("denial not audited")
	}
	if strings.Contains(strings.Join(index.outcomes, " "), "fixture") {
		t.Fatal("target leaked into audit")
	}
}

func TestAccountLookupAuthorization(t *testing.T) {
	for _, tc := range []struct {
		name  string
		user  *auth.User
		admin bool
		err   error
		want  int
	}{
		{"anonymous", nil, false, nil, 403},
		{"customer", &auth.User{ID: "customer", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, false, nil, 403},
		{"revoked", &auth.User{ID: "admin", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, false, nil, 403},
		{"user key admin", &auth.User{ID: "admin", AZP: "ukey-test", PrincipalType: auth.PrincipalTypeUserKey}, true, nil, 403},
		{"CLI admin", &auth.User{ID: "admin", AZP: "cua-cli", PrincipalType: auth.PrincipalTypeUser}, true, nil, 403},
		{"automation spoof", &auth.User{ID: "admin", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeGitHubOIDC}, true, nil, 403},
		{"provider unavailable", &auth.User{ID: "admin", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, false, errors.New("private error"), 503},
		{"no database", &auth.User{ID: "admin", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}, true, nil, 503},
	} {
		t.Run(tc.name, func(t *testing.T) {
			h := Handlers{adminAccessEvaluator: func(context.Context, *auth.User) (bool, error) { return tc.admin, tc.err }}
			r := httptest.NewRequest("POST", "/api/admin/account-lookup", strings.NewReader(`{"kind":"account_id","value":"private-target"}`))
			if tc.user != nil {
				r = r.WithContext(context.WithValue(r.Context(), auth.UserKey, tc.user))
			}
			w := httptest.NewRecorder()
			h.LookupAccount(w, r)
			if w.Code != tc.want || w.Header().Get("Cache-Control") != "no-store" {
				t.Fatalf("status=%d headers=%v", w.Code, w.Header())
			}
			if strings.Contains(w.Body.String(), "private") {
				t.Fatal("sensitive data in error")
			}
		})
	}
}

func TestAccountLookupAuditFailureOverridesInvalidInput(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	index := &lookupTestIndex{auditErr: errors.New("private audit failure")}
	s := accountlookup.New(ctx, index, lookupTestDirectory{}, "fixture", "fixture-key", nil)
	defer func() { cancel(); s.Wait() }()
	h := Handlers{AccountLookup: s, adminAccessEvaluator: func(context.Context, *auth.User) (bool, error) { return true, nil }}
	r := httptest.NewRequest("POST", "/api/admin/account-lookup", strings.NewReader(`{}`))
	r = r.WithContext(context.WithValue(r.Context(), auth.UserKey, &auth.User{ID: "admin", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}))
	w := httptest.NewRecorder()
	h.LookupAccount(w, r)
	if w.Code != http.StatusServiceUnavailable || strings.Contains(w.Body.String(), "private") {
		t.Fatalf("audit failure response=%d %s", w.Code, w.Body.String())
	}
}
