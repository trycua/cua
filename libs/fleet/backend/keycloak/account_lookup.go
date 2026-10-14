package keycloak

import (
	"context"
	"errors"
	"net/http"
	"time"

	"cyclops-cs-backend/internal/redactederror"

	"github.com/Nerzal/gocloak/v13"
)

var ErrAccountDirectory = errors.New("account directory unavailable")

type Account struct {
	ID            string     `json:"id"`
	Username      string     `json:"username"`
	Email         string     `json:"email"`
	EmailVerified bool       `json:"email_verified"`
	CreatedAt     *time.Time `json:"created_at,omitempty"`
}

// Keep directory response bodies and identifiers out of errors and traces.
func (a *Admin) LookupAccount(ctx context.Context, subject string) (*Account, error) {
	token, err := a.accountLookupToken(ctx)
	if err != nil {
		return nil, redactederror.New(ErrAccountDirectory.Error(), errors.Join(ErrAccountDirectory, err))
	}
	u, err := a.client.GetUserByID(ctx, token, a.realm, subject)
	if err != nil {
		var apiErr *gocloak.APIError
		if errors.As(err, &apiErr) && apiErr.Code == http.StatusNotFound {
			return nil, nil
		}
		return nil, redactederror.New(ErrAccountDirectory.Error(), errors.Join(ErrAccountDirectory, err))
	}
	if u == nil || derefStr(u.ID) != subject {
		return nil, ErrAccountDirectory
	}
	result := &Account{ID: subject, Username: derefStr(u.Username), Email: derefStr(u.Email), EmailVerified: u.EmailVerified != nil && *u.EmailVerified}
	if u.CreatedTimestamp != nil {
		t := time.UnixMilli(*u.CreatedTimestamp).UTC()
		result.CreatedAt = &t
	}
	return result, nil
}

// ListAccountIDs is used only by the explicit offline backfill, never a lookup.
func (a *Admin) ListAccountIDs(ctx context.Context, first, max int) ([]string, error) {
	if first < 0 || max < 1 || max > 100 {
		return nil, ErrAccountDirectory
	}
	token, err := a.accountLookupToken(ctx)
	if err != nil {
		return nil, redactederror.New(ErrAccountDirectory.Error(), errors.Join(ErrAccountDirectory, err))
	}
	users, err := a.client.GetUsers(ctx, token, a.realm, gocloak.GetUsersParams{First: &first, Max: &max, BriefRepresentation: gocloak.BoolP(true)})
	if err != nil {
		return nil, redactederror.New(ErrAccountDirectory.Error(), errors.Join(ErrAccountDirectory, err))
	}
	ids := make([]string, 0, len(users))
	for _, u := range users {
		if u == nil || derefStr(u.ID) == "" {
			return nil, ErrAccountDirectory
		}
		ids = append(ids, *u.ID)
	}
	return ids, nil
}

func (a *Admin) accountLookupToken(ctx context.Context) (string, error) {
	// Unlike the general admin helper, do not record upstream error bodies.
	token, err := a.client.LoginClient(ctx, a.clientID, a.clientSecret, a.realm)
	if err != nil {
		return "", redactederror.New(ErrAccountDirectory.Error(), errors.Join(ErrAccountDirectory, err))
	}
	if token == nil || token.AccessToken == "" {
		return "", ErrAccountDirectory
	}
	return token.AccessToken, nil
}
