package handlers

import (
	"encoding/json"
	"net/http"
)

// CreateUserKeyRequest is the body for POST /api/user-keys.
type CreateUserKeyRequest struct {
	Name  string   `json:"name" example:"my-ci-key"`
	Scope []string `json:"scope" example:"[\"ns1\",\"ns2\"]"`
}

// CreateUserKeyResponse is returned on successful creation. The
// `client_secret` is shown exactly ONCE and cannot be retrieved later.
type CreateUserKeyResponse struct {
	ClientID     string   `json:"client_id"`
	ClientSecret string   `json:"client_secret"`
	TokenURL     string   `json:"token_url"`
	Name         string   `json:"name"`
	Scope        []string `json:"scope"`
}

// UserKeyResponse is a single key in the list response (no secret).
type UserKeyResponse struct {
	ID       string   `json:"id"`
	ClientID string   `json:"client_id"`
	Name     string   `json:"name"`
	Scope    []string `json:"scope"`
}

// ListUserKeysResponse wraps the user keys list.
type ListUserKeysResponse struct {
	Keys []UserKeyResponse `json:"keys"`
}

// CreateUserKey godoc
//
//	@Summary		Create a per-user API key
//	@Description	Creates a Keycloak service-account client that acts on behalf of the calling user. The client_secret is returned exactly once; it cannot be retrieved later.
//	@Tags			user-keys
//	@Accept			json
//	@Produce		json
//	@Param			body	body		CreateUserKeyRequest	true	"Key parameters"
//	@Success		201		{object}	CreateUserKeyResponse
//	@Failure		400		{object}	ErrorResponse
//	@Failure		401		{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/user-keys [post]
func (h Handlers) CreateUserKey(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	var req CreateUserKeyRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid body")
		return
	}
	if req.Name == "" {
		writeErr(w, http.StatusBadRequest, "name is required")
		return
	}
	// Validate scope entries if provided.
	for _, ns := range req.Scope {
		if !dnsLabel.MatchString(ns) || len(ns) > 63 {
			writeErr(w, http.StatusBadRequest, "each scope entry must be a DNS-1123 label")
			return
		}
	}

	cid, secret, tokenURL, err := h.Admin.CreateUserKeyClient(r.Context(), req.Name, user.ID, req.Scope)
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "keycloak: "+err.Error())
		return
	}
	writeJSON(w, http.StatusCreated, CreateUserKeyResponse{
		ClientID:     cid,
		ClientSecret: secret,
		TokenURL:     tokenURL,
		Name:         req.Name,
		Scope:        req.Scope,
	})
}

// ListUserKeys godoc
//
//	@Summary		List the calling user's API keys
//	@Tags			user-keys
//	@Produce		json
//	@Success		200	{object}	ListUserKeysResponse
//	@Failure		401	{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/user-keys [get]
func (h Handlers) ListUserKeys(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}

	keys, err := h.Admin.ListUserKeyClients(r.Context(), user.ID)
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "keycloak: "+err.Error())
		return
	}

	resp := make([]UserKeyResponse, 0, len(keys))
	for _, k := range keys {
		resp = append(resp, UserKeyResponse{
			ID:       k.ID,
			ClientID: k.ClientID,
			Name:     k.Name,
			Scope:    k.Scope,
		})
	}
	writeJSON(w, http.StatusOK, ListUserKeysResponse{Keys: resp})
}

// DeleteUserKey godoc
//
//	@Summary		Revoke a per-user API key
//	@Tags			user-keys
//	@Produce		json
//	@Param			id	path	string	true	"Keycloak client UUID"
//	@Success		204
//	@Failure		401	{object}	ErrorResponse
//	@Failure		403	{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/user-keys/{id} [delete]
func (h Handlers) DeleteUserKey(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	id := r.PathValue("id")
	if id == "" {
		writeErr(w, http.StatusBadRequest, "id is required")
		return
	}

	if err := h.Admin.DeleteUserKeyClient(r.Context(), user.ID, id); err != nil {
		writeErr(w, http.StatusForbidden, err.Error())
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

