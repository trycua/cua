package keycloak

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/Nerzal/gocloak/v13"
)

func TestAccountDirectoryExactAndSanitized(t *testing.T) {
	for _, code := range []int{200, 404, 403, 500} {
		t.Run(fmt.Sprint(code), func(t *testing.T) {
			calls := 0
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.Header().Set("Content-Type", "application/json")
				if strings.HasSuffix(r.URL.Path, "/token") {
					fmt.Fprint(w, `{"access_token":"synthetic-token","expires_in":60}`)
					return
				}
				calls++
				if r.URL.Path != "/admin/realms/test/users/test-account" {
					t.Errorf("not exact path: %s", r.URL.Path)
				}
				w.WriteHeader(code)
				if code == 200 {
					fmt.Fprint(w, `{"id":"test-account","username":"example","email":"example@example.com","emailVerified":true,"createdTimestamp":1000}`)
				} else {
					fmt.Fprint(w, `{"errorMessage":"PRIVATE-DIRECTORY-DETAIL"}`)
				}
			}))
			defer server.Close()
			a := NewAdmin(server.URL, "test", "test-client", "synthetic-secret", "key-", "ukey-")
			account, err := a.LookupAccount(context.Background(), "test-account")
			if code == 200 && (err != nil || account == nil || account.Username != "example" || account.CreatedAt == nil) {
				t.Fatalf("account=%+v err=%v", account, err)
			}
			if code == 404 && (err != nil || account != nil) {
				t.Fatal("404 must be absent account")
			}
			if code >= 400 && code != 404 {
				var apiErr *gocloak.APIError
				if !errors.Is(err, ErrAccountDirectory) || err.Error() != ErrAccountDirectory.Error() || !errors.As(err, &apiErr) || apiErr.Code != code || errors.Unwrap(err) == nil {
					t.Fatal("directory failure must redact text and retain the API error")
				}
			}
			if calls != 1 {
				t.Fatal("not a single exact lookup")
			}
		})
	}
}
