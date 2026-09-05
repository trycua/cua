package handlers

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"time"

	"cyclops-cs-backend/accountlookup"
	"cyclops-cs-backend/auth"
)

func PrivateAccountLookup(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		w.Header().Set("Pragma", "no-cache")
		next.ServeHTTP(w, r)
	})
}

// Runs after token validation but before policy, so denied principals are audited
// without decoding their request bodies or copying them to ordinary logs.
func (h Handlers) AuditAccountLookupAccess(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		captured := &statusCapture{ResponseWriter: w}
		next.ServeHTTP(captured, r)
		if captured.statusCode == http.StatusForbidden {
			if u := currentUser(r); u != nil {
				ctx, cancel := context.WithTimeout(context.WithoutCancel(r.Context()), time.Second)
				defer cancel()
				h.AccountLookup.AuditDenied(ctx, u.ID)
			}
		}
	})
}

// LookupAccount discloses one account only to a freshly authorized human admin.
func (h Handlers) LookupAccount(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Pragma", "no-cache")
	u := currentUser(r)
	if u == nil || u.ID == "" || u.PrincipalType != auth.PrincipalTypeUser || u.AZP != "cyclops-cs-spa" {
		writeErr(w, http.StatusForbidden, "account lookup requires a human admin session")
		return
	}
	allowed, err := h.isAdmin(r.Context(), u)
	if err != nil {
		writeErr(w, http.StatusServiceUnavailable, "account lookup authorization unavailable")
		return
	}
	if !allowed {
		writeErr(w, http.StatusForbidden, "account lookup requires a human admin session")
		return
	}
	if r.Method != http.MethodPost {
		writeErr(w, http.StatusMethodNotAllowed, "POST required")
		return
	}
	var input accountlookup.Request
	d := json.NewDecoder(http.MaxBytesReader(w, r.Body, 1024))
	d.DisallowUnknownFields()
	if d.Decode(&input) != nil || d.Decode(&struct{}{}) != io.EOF {
		// Invalid input is still rate limited and audited by the service.
		input = accountlookup.Request{}
	}
	ctx, cancel := context.WithTimeout(r.Context(), 5*time.Second)
	defer cancel()
	result, err := h.AccountLookup.Lookup(ctx, u.ID, input)
	switch {
	case errors.Is(err, accountlookup.ErrUnavailable):
		writeErr(w, http.StatusServiceUnavailable, "account lookup unavailable")
	case errors.Is(err, accountlookup.ErrInvalid):
		writeErr(w, http.StatusBadRequest, "provide one exact pseudonym or account ID")
	case errors.Is(err, accountlookup.ErrRateLimited):
		w.Header().Set("Retry-After", "60")
		writeErr(w, http.StatusTooManyRequests, "account lookup limit reached; try again in a minute")
	case err != nil:
		writeErr(w, http.StatusServiceUnavailable, "account lookup unavailable")
	default:
		writeJSON(w, http.StatusOK, result)
	}
}
