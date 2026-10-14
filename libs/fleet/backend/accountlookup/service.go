// Package accountlookup resolves analytics identities only inside the admin boundary.
package accountlookup

import (
	"context"
	"errors"
	"net/http"
	"regexp"
	"sync"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/internal/redactederror"
	"cyclops-cs-backend/keycloak"
	"cyclops-cs-backend/productanalytics"
)

var (
	ErrUnavailable   = errors.New("account lookup unavailable")
	ErrRateLimited   = errors.New("account lookup rate limited")
	ErrInvalid       = errors.New("invalid exact account lookup")
	pseudonymPattern = regexp.MustCompile(`^u_[0-9a-f]{64}$`)
	accountPattern   = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._:@-]{0,254}$`)
)

type Index interface {
	Record(context.Context, string, string, string, string) error
	Resolve(context.Context, string, string, string) (string, bool, error)
	Complete(context.Context, string, string) (bool, error)
	Audit(context.Context, string, string) error
	Allow(context.Context, string) (bool, error)
}

type Directory interface {
	LookupAccount(context.Context, string) (*keycloak.Account, error)
}
type Request struct {
	Kind  string `json:"kind"`
	Value string `json:"value"`
}
type Account struct {
	keycloak.Account
	Pseudonym     string                         `json:"pseudonym"`
	IdentityClass productanalytics.IdentityClass `json:"identity_class"`
	Excluded      bool                           `json:"excluded"`
}
type Result struct {
	Status           string   `json:"status"`
	BackfillComplete bool     `json:"backfill_complete"`
	Account          *Account `json:"account,omitempty"`
}

type Service struct {
	index             Index
	directory         Directory
	realm, key, keyID string
	excluded          map[string]bool
	queue             chan string
	wg                sync.WaitGroup
}

// KeyID scopes private index rows to the configured key without storing the key.
func KeyID(key string) string {
	return productanalytics.PseudonymForUserID("account-lookup-key-version-v1", key)
}

func New(ctx context.Context, index Index, directory Directory, realm, key string, excluded []string) *Service {
	s := &Service{index: index, directory: directory, realm: realm, key: key, keyID: KeyID(key), excluded: map[string]bool{}, queue: make(chan string, 256)}
	for _, id := range excluded {
		s.excluded[id] = true
	}
	s.wg.Add(1)
	go s.run(ctx)
	return s
}

func (s *Service) Wait() { s.wg.Wait() }

// Denials never disclose an account, even when their best-effort audit fails.
func (s *Service) AuditDenied(ctx context.Context, actor string) {
	if s != nil && s.index != nil && actor != "" {
		_ = s.index.Audit(ctx, actor, "forbidden")
	}
}

func (s *Service) Lookup(ctx context.Context, actor string, input Request) (Result, error) {
	if s == nil || s.index == nil || s.directory == nil || s.key == "" || s.realm == "" {
		return Result{}, ErrUnavailable
	}
	allowed, err := s.index.Allow(ctx, actor)
	if err != nil {
		return Result{}, redactederror.New(ErrUnavailable.Error(), errors.Join(ErrUnavailable, err))
	}
	if !allowed {
		return s.finish(ctx, actor, "rate_limited", Result{}, ErrRateLimited)
	}
	if !((input.Kind == "pseudonym" && pseudonymPattern.MatchString(input.Value)) || (input.Kind == "account_id" && accountPattern.MatchString(input.Value))) {
		return s.finish(ctx, actor, "invalid", Result{}, ErrInvalid)
	}
	complete, err := s.index.Complete(ctx, s.realm, s.keyID)
	if err != nil {
		return s.finish(ctx, actor, "unavailable", Result{}, redactederror.New(ErrUnavailable.Error(), errors.Join(ErrUnavailable, err)))
	}
	result := Result{BackfillComplete: complete}
	subject := input.Value
	if input.Kind == "pseudonym" {
		var found bool
		subject, found, err = s.index.Resolve(ctx, s.realm, s.keyID, input.Value)
		if err != nil {
			return s.finish(ctx, actor, "unavailable", Result{}, redactederror.New(ErrUnavailable.Error(), errors.Join(ErrUnavailable, err)))
		}
		if !found {
			result.Status = "mapping_missing"
			return s.finish(ctx, actor, result.Status, result, nil)
		}
		if productanalytics.PseudonymForUserID(subject, s.key) != input.Value {
			return s.finish(ctx, actor, "unavailable", Result{}, ErrUnavailable)
		}
	}
	account, err := s.directory.LookupAccount(ctx, subject)
	if err != nil {
		return s.finish(ctx, actor, "unavailable", Result{}, redactederror.New(ErrUnavailable.Error(), errors.Join(ErrUnavailable, err)))
	}
	if account == nil {
		result.Status = "account_missing"
		return s.finish(ctx, actor, result.Status, result, nil)
	}
	if account.ID != subject {
		return s.finish(ctx, actor, "unavailable", Result{}, ErrUnavailable)
	}
	result.Status = "found"
	result.Account = &Account{Account: *account, Pseudonym: productanalytics.PseudonymForUserID(subject, s.key), IdentityClass: productanalytics.ClassifyIdentity(&auth.User{ID: subject, Email: account.Email, EmailVerified: account.EmailVerified}), Excluded: s.excluded[subject]}
	return s.finish(ctx, actor, result.Status, result, nil)
}

// Do not disclose identity when the restricted audit cannot be committed.
func (s *Service) finish(ctx context.Context, actor, outcome string, result Result, err error) (Result, error) {
	if auditErr := s.index.Audit(ctx, actor, outcome); auditErr != nil {
		return Result{}, redactederror.New(ErrUnavailable.Error(), errors.Join(ErrUnavailable, err, auditErr))
	}
	return result, err
}

// Observe runs after token validation/owner normalization. The bounded queue
// never blocks authentication or workloads, including during a DB outage.
func (s *Service) Observe(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		u := auth.GetUser(r.Context())
		if s != nil && s.key != "" && u != nil && u.ID != "" && ((u.PrincipalType == auth.PrincipalTypeUser && (u.AZP == "cyclops-cs-spa" || u.AZP == "cua-cli" || u.AZP == "cua-desktop")) || u.PrincipalType == auth.PrincipalTypeUserKey) {
			select {
			case s.queue <- u.ID:
			default:
			}
		}
		next.ServeHTTP(w, r)
	})
}

func (s *Service) run(ctx context.Context) {
	defer s.wg.Done()
	seen := map[string]time.Time{}
	for {
		select {
		case <-ctx.Done():
			return
		case subject := <-s.queue:
			if time.Now().Before(seen[subject]) {
				continue
			}
			if s.index == nil || s.key == "" {
				continue
			}
			callCtx, cancel := context.WithTimeout(ctx, 2*time.Second)
			err := s.index.Record(callCtx, s.realm, s.keyID, productanalytics.PseudonymForUserID(subject, s.key), subject)
			cancel()
			if err == nil {
				if len(seen) >= 4096 {
					clear(seen)
				}
				seen[subject] = time.Now().Add(time.Hour)
			}
		}
	}
}
