package auth

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestAccountLookupFreshAuthorization(t *testing.T) {
	LoadOpa()
	original := computeFreshAdminFlagsFn
	t.Cleanup(func() { computeFreshAdminFlagsFn = original })
	members := []interface{}{"test-admin"}
	calls := 0
	computeFreshAdminFlagsFn = func(context.Context) map[string]interface{} {
		calls++
		return map[string]interface{}{"admin_subs": members}
	}
	policy := surfacePolicies["account-lookup"]
	h := PolicyMiddleware(policy.tree(), policy.options...)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		ok, err := EvalIsAdminFresh(r.Context(), GetUser(r.Context()))
		if err != nil || !ok {
			t.Fatal("fresh handler check failed")
		}
		w.WriteHeader(204)
	}))
	for _, tc := range []struct {
		name, azp, principal string
		revoke               bool
		want                 int
	}{
		{"admin", "cyclops-cs-spa", PrincipalTypeUser, false, 204},
		{"CLI", "cua-cli", PrincipalTypeUser, false, 403},
		{"user key", "ukey-test", PrincipalTypeUserKey, false, 403},
		{"automation", "cyclops-cs-spa", PrincipalTypeGitHubOIDC, false, 403},
		{"revoked", "cyclops-cs-spa", PrincipalTypeUser, true, 403},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if tc.revoke {
				members = []interface{}{}
			}
			r := routeRequest("POST", "/api/admin/account-lookup", "/api/admin/account-lookup", nil, "")
			r = r.WithContext(context.WithValue(r.Context(), UserKey, &User{ID: "test-admin", AZP: tc.azp, PrincipalType: tc.principal}))
			before := calls
			w := httptest.NewRecorder()
			h.ServeHTTP(w, r)
			if w.Code != tc.want || calls != before+1 {
				t.Fatalf("status=%d fresh calls=%d", w.Code, calls-before)
			}
		})
	}
}
