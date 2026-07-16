package handlers

import (
	"encoding/json"
	"net/http"

	"cyclops-cs-backend/keycloak"
)

// CreateKeyRequest is the body for POST /api/keys.
type CreateKeyRequest struct {
	Name      string `json:"name" example:"ci-prod"`
	Namespace string `json:"namespace" example:"test-pool"`
}

// CreateKeyResponse is what the SPA shows once after a successful create.
// `client_secret` is irretrievable after this response — the SPA must
// surface it to the user immediately and forget it.
type CreateKeyResponse struct {
	ClientID     string `json:"client_id"`
	ClientSecret string `json:"client_secret"`
	TokenURL     string `json:"token_url"`
	Name         string `json:"name"`
	Namespace    string `json:"namespace"`
}

// ListKeysResponse wraps the keys list — keeps room to add pagination
// metadata later without a breaking change to the client.
type ListKeysResponse struct {
	Keys []keycloak.KeyClient `json:"keys"`
}

// ErrorResponse is the shape every 4xx/5xx body uses.
type ErrorResponse struct {
	Error string `json:"error"`
}

// CreateKey godoc
//
//	@Summary		Create a new API key
//	@Description	Creates a Keycloak service-account client owned by the calling user. The returned `client_secret` is shown exactly once.
//	@Tags			keys
//	@Accept			json
//	@Produce		json
//	@Param			body	body		CreateKeyRequest	true	"Key parameters"
//	@Success		201		{object}	CreateKeyResponse
//	@Failure		400		{object}	ErrorResponse
//	@Failure		401		{object}	ErrorResponse
//	@Failure		403		{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/keys [post]
func (h Handlers) CreateKey(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	var req CreateKeyRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid body")
		return
	}
	if req.Name == "" {
		writeErr(w, http.StatusBadRequest, "name is required")
		return
	}
	if !dnsLabel.MatchString(req.Namespace) || len(req.Namespace) > 63 {
		writeErr(w, http.StatusBadRequest, "namespace must be a DNS-1123 label")
		return
	}
	cid, secret, _, err := h.Admin.CreateKeyClient(r.Context(), req.Name, req.Namespace, user.ID)
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "keycloak: "+err.Error())
		return
	}
	writeJSON(w, http.StatusCreated, CreateKeyResponse{
		ClientID:     cid,
		ClientSecret: secret,
		TokenURL:     h.KC.TokenURL,
		Name:         req.Name,
		Namespace:    req.Namespace,
	})
}

// ListKeys godoc
//
//	@Summary		List the calling user's API keys
//	@Tags			keys
//	@Produce		json
//	@Success		200	{object}	ListKeysResponse
//	@Failure		401	{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/keys [get]
func (h Handlers) ListKeys(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	keys, err := h.Admin.ListKeyClients(r.Context(), user.ID)
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "keycloak: "+err.Error())
		return
	}
	writeJSON(w, http.StatusOK, ListKeysResponse{Keys: keys})
}

// DeleteKey godoc
//
//	@Summary		Revoke an API key by Keycloak client UUID
//	@Tags			keys
//	@Produce		json
//	@Param			id	path	string	true	"Keycloak client UUID"
//	@Success		204
//	@Failure		401	{object}	ErrorResponse
//	@Failure		403	{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/keys/{id} [delete]
func (h Handlers) DeleteKey(w http.ResponseWriter, r *http.Request) {
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
	if err := h.Admin.DeleteKeyClient(r.Context(), id, user.ID); err != nil {
		writeErr(w, http.StatusForbidden, err.Error())
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
