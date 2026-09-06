package accountlookup

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/keycloak"
	"cyclops-cs-backend/productanalytics"
	"github.com/trycua/cloud/pkg/featureflags"
)

func TestMain(m *testing.M) {
	if err := os.Setenv("CYCLOPS_CS_ADMIN_SUBS", `[]`); err != nil {
		panic(err)
	}
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		panic(err)
	}
	os.Exit(m.Run())
}

type fakeIndex struct {
	subject           string
	complete, limited bool
	err, auditErr     error
	outcomes          []string
	recorded          chan string
}

func (f *fakeIndex) Record(_ context.Context, _, _, pseudo, subject string) error {
	if f.recorded != nil {
		f.recorded <- pseudo + ":" + subject
	}
	return f.err
}
func (f *fakeIndex) Resolve(context.Context, string, string, string) (string, bool, error) {
	return f.subject, f.subject != "", f.err
}
func (f *fakeIndex) Complete(context.Context, string, string) (bool, error) { return f.complete, f.err }
func (f *fakeIndex) Allow(context.Context, string) (bool, error)            { return !f.limited, f.err }
func (f *fakeIndex) Audit(_ context.Context, actor, outcome string) error {
	f.outcomes = append(f.outcomes, actor+":"+outcome)
	return f.auditErr
}

type fakeDirectory struct {
	account *keycloak.Account
	err     error
	calls   int
}

type privateLookupError struct{}

func (*privateLookupError) Error() string { return "private upstream detail" }

func TestFinishPreservesIncomingAndAuditErrors(t *testing.T) {
	for _, auditFails := range []bool{false, true} {
		cause := &privateLookupError{}
		auditCause := errors.New("private audit detail")
		index := &fakeIndex{}
		if auditFails {
			index.auditErr = auditCause
		}
		s := testService(t, index, &fakeDirectory{})
		_, err := s.finish(context.Background(), "actor", "unavailable", Result{}, cause)
		var typed *privateLookupError
		if !errors.Is(err, cause) || !errors.As(err, &typed) || typed != cause {
			t.Fatal("finish discarded incoming error")
		}
		if auditFails {
			if err.Error() != ErrUnavailable.Error() || !errors.Is(err, ErrUnavailable) || !errors.Is(err, auditCause) || errors.Unwrap(err) == nil {
				t.Fatal("audit failure must redact text and preserve both causes")
			}
		} else if err != cause {
			t.Fatal("successful audit must return incoming error unchanged")
		}
	}
}

func TestLookupPreservesPrivateCause(t *testing.T) {
	cause := &privateLookupError{}
	for _, fromDirectory := range []bool{false, true} {
		index, directory := &fakeIndex{}, &fakeDirectory{}
		if fromDirectory {
			directory.err = cause
		} else {
			index.err = cause
		}
		s := testService(t, index, directory)
		result, err := s.Lookup(context.Background(), "actor", Request{"account_id", "test-account"})
		var typed *privateLookupError
		if result.Account != nil || err == nil || err.Error() != ErrUnavailable.Error() || !errors.Is(err, ErrUnavailable) || !errors.Is(err, cause) || !errors.As(err, &typed) || typed != cause || errors.Unwrap(err) == nil {
			t.Fatal("lookup must redact text and preserve classification and cause")
		}
	}
}

func (f *fakeDirectory) LookupAccount(context.Context, string) (*keycloak.Account, error) {
	f.calls++
	return f.account, f.err
}
func testService(t *testing.T, index *fakeIndex, directory *fakeDirectory) *Service {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	s := New(ctx, index, directory, "test-realm", "synthetic-test-key", []string{"excluded-account"})
	t.Cleanup(func() { cancel(); s.Wait() })
	return s
}

func TestLookupExactMatchAndCurrentClassification(t *testing.T) {
	index := &fakeIndex{subject: "test-account"}
	directory := &fakeDirectory{account: &keycloak.Account{ID: "test-account", Email: "test@trycua.com", EmailVerified: true}}
	s := testService(t, index, directory)
	pseudo := productanalytics.PseudonymForUserID("test-account", "synthetic-test-key")
	for _, input := range []Request{{"pseudonym", pseudo}, {"account_id", "test-account"}} {
		got, err := s.Lookup(context.Background(), "test-admin", input)
		if err != nil || got.Status != "found" || got.Account.ID != "test-account" || got.Account.Pseudonym != pseudo || got.Account.IdentityClass != productanalytics.IdentityInternal {
			t.Fatalf("unexpected exact lookup: %+v %v", got, err)
		}
	}
	if directory.calls != 2 {
		t.Fatal("lookup must use one exact directory call")
	}
	if strings.Contains(strings.Join(index.outcomes, " "), "test-account") {
		t.Fatal("target leaked into audit")
	}
	directory.account.EmailVerified = false
	got, err := s.Lookup(context.Background(), "test-admin", Request{"account_id", "test-account"})
	if err != nil || got.Account.IdentityClass != productanalytics.IdentityExternal {
		t.Fatal("unverified domain must not imply internal")
	}
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", `["test-account"]`)
	auth.InvalidateFeatureFlags()
	t.Cleanup(auth.InvalidateFeatureFlags)
	got, err = s.Lookup(context.Background(), "test-admin", Request{"account_id", "test-account"})
	if err != nil || got.Account.IdentityClass != productanalytics.IdentityInternal {
		t.Fatal("trusted admin membership must imply internal independently of email")
	}
}

func TestLookupFailuresDoNotDiscloseAccounts(t *testing.T) {
	for _, tc := range []struct {
		name   string
		index  fakeIndex
		dir    fakeDirectory
		input  Request
		status string
		err    error
		calls  int
	}{
		{"missing mapping", fakeIndex{}, fakeDirectory{}, Request{"pseudonym", "u_" + strings.Repeat("0", 64)}, "mapping_missing", nil, 0},
		{"scanned missing mapping", fakeIndex{complete: true}, fakeDirectory{}, Request{"pseudonym", "u_" + strings.Repeat("0", 64)}, "mapping_missing", nil, 0},
		{"deleted", fakeIndex{}, fakeDirectory{}, Request{"account_id", "deleted"}, "account_missing", nil, 1},
		{"directory down", fakeIndex{}, fakeDirectory{err: errors.New("sensitive upstream detail")}, Request{"account_id", "test-account"}, "", ErrUnavailable, 1},
		{"index down", fakeIndex{err: errors.New("db secret")}, fakeDirectory{}, Request{"account_id", "test-account"}, "", ErrUnavailable, 0},
		{"audit down", fakeIndex{auditErr: errors.New("audit down")}, fakeDirectory{account: &keycloak.Account{ID: "test-account"}}, Request{"account_id", "test-account"}, "", ErrUnavailable, 1},
		{"limited", fakeIndex{limited: true}, fakeDirectory{}, Request{"account_id", "test-account"}, "", ErrRateLimited, 0},
		{"bad kind", fakeIndex{}, fakeDirectory{}, Request{"email", "test@example.com"}, "", ErrInvalid, 0},
		{"traversal", fakeIndex{}, fakeDirectory{}, Request{"account_id", "../users"}, "", ErrInvalid, 0},
		{"oversize", fakeIndex{}, fakeDirectory{}, Request{"account_id", strings.Repeat("a", 256)}, "", ErrInvalid, 0},
		{"corrupt index", fakeIndex{subject: "wrong"}, fakeDirectory{}, Request{"pseudonym", "u_" + strings.Repeat("0", 64)}, "", ErrUnavailable, 0},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s := testService(t, &tc.index, &tc.dir)
			got, err := s.Lookup(context.Background(), "test-admin", tc.input)
			if !errors.Is(err, tc.err) || got.Status != tc.status || got.Account != nil || tc.dir.calls != tc.calls {
				t.Fatalf("result=%+v err=%v calls=%d", got, err, tc.dir.calls)
			}
		})
	}
}

func TestObserverNormalizesHumanAndUserKeyIdentity(t *testing.T) {
	for _, principal := range []struct {
		kind, azp string
		want      bool
	}{
		{auth.PrincipalTypeUser, "cyclops-cs-spa", true}, {auth.PrincipalTypeUser, "cua-cli", true},
		{auth.PrincipalTypeUserKey, "ukey-synthetic", true}, {auth.PrincipalTypeUser, "key-synthetic", false},
		{auth.PrincipalTypeUserKey, "custom-user-key-synthetic", true},
		{auth.PrincipalTypeGitHubOIDC, "github-actions", false},
	} {
		t.Run(principal.azp, func(t *testing.T) {
			index := &fakeIndex{recorded: make(chan string, 1)}
			s := testService(t, index, &fakeDirectory{})
			h := s.Observe(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(204) }))
			r := httptest.NewRequest("GET", "/api/config", nil)
			r = r.WithContext(context.WithValue(r.Context(), auth.UserKey, &auth.User{ID: "same-owner", AZP: principal.azp, PrincipalType: principal.kind}))
			w := httptest.NewRecorder()
			h.ServeHTTP(w, r)
			if w.Code != 204 {
				t.Fatal(w.Code)
			}
			if principal.want {
				select {
				case got := <-index.recorded:
					if got != productanalytics.PseudonymForUserID("same-owner", "synthetic-test-key")+":same-owner" {
						t.Fatal("identity changed")
					}
				case <-time.After(time.Second):
					t.Fatal("mapping not recorded")
				}
			} else if len(s.queue) != 0 || len(index.recorded) != 0 {
				t.Fatal("machine principal observed")
			}
		})
	}
}

func TestObserverFullQueueDoesNotBlock(t *testing.T) {
	s := &Service{key: "test", queue: make(chan string, 1)}
	s.queue <- "full"
	r := httptest.NewRequest("GET", "/api/config", nil)
	r = r.WithContext(context.WithValue(r.Context(), auth.UserKey, &auth.User{ID: "owner", AZP: "cyclops-cs-spa", PrincipalType: auth.PrincipalTypeUser}))
	done := make(chan struct{})
	go func() {
		s.Observe(http.HandlerFunc(func(http.ResponseWriter, *http.Request) {})).ServeHTTP(httptest.NewRecorder(), r)
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("observer blocked request")
	}
}
