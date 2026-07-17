package handlers

import (
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"regexp"

	"cyclops-cs-backend/templates"
)

// templateName bounds a template's name: 1–63 chars, alphanumerics plus
// dot/dash/underscore, no leading separator. It deliberately forbids ':'
// (the Redis key separator) so a name can never escape its owner's
// keyspace. Looser than a DNS-1123 label — a template name is a friendly
// SPA label, not a K8s object name.
var templateName = regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9._-]{0,62}$`)

// maxTemplateConfigBytes caps the stored config blob. A pool config is a
// few hundred bytes; 256 KiB is a generous ceiling that still bounds what
// a single request can push into Redis.
const maxTemplateConfigBytes = 256 << 10

// CreatePoolTemplateRequest is the body for POST /api/pool-templates.
// Config is the opaque pool config the SPA would otherwise pass straight
// to createPool (cpu, ram, ociImage, replicas, services, probes).
type CreatePoolTemplateRequest struct {
	Name   string          `json:"name" example:"gpu-large"`
	Config json.RawMessage `json:"config" swaggertype:"object"`
}

// PoolTemplateResponse is a single saved template. It mirrors the four
// stored fields: the owning user, the name, when it was created, and the
// opaque pool config.
type PoolTemplateResponse struct {
	User      string          `json:"user"`
	Name      string          `json:"name"`
	CreatedAt string          `json:"createdAt"`
	Config    json.RawMessage `json:"config" swaggertype:"object"`
}

func toPoolTemplateResponse(t *templates.Template) PoolTemplateResponse {
	return PoolTemplateResponse{
		User:      t.User,
		Name:      t.Name,
		CreatedAt: t.CreatedAt.UTC().Format("2006-01-02T15:04:05.999999999Z07:00"),
		Config:    t.Config,
	}
}

// templatesEnabled writes a 503 and returns false when no Redis-backed
// store is configured. The routes are always registered; this keeps the
// failure mode explicit ("not configured") instead of silently empty.
func (h Handlers) templatesEnabled(w http.ResponseWriter) bool {
	if h.Templates == nil {
		writeErr(w, http.StatusServiceUnavailable, "pool templates are not configured")
		return false
	}
	return true
}

// CreatePoolTemplate godoc
//
//	@Summary		Save a pool config as a reusable template
//	@Description	Stores the given pool config under a name owned by the calling user. Re-saving an existing name overwrites the config while preserving the original creation time. Use GET /api/pool-templates to list them and seed a new pool.
//	@Tags			pool-templates
//	@Accept			json
//	@Produce		json
//	@Param			body	body		CreatePoolTemplateRequest	true	"Template name + pool config"
//	@Success		201		{object}	PoolTemplateResponse
//	@Failure		400		{object}	ErrorResponse
//	@Failure		401		{object}	ErrorResponse
//	@Failure		503		{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/pool-templates [post]
func (h Handlers) CreatePoolTemplate(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	if !h.templatesEnabled(w) {
		return
	}

	r.Body = http.MaxBytesReader(w, r.Body, maxTemplateConfigBytes+4096)
	var req CreatePoolTemplateRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		var mbe *http.MaxBytesError
		if errors.As(err, &mbe) {
			writeErr(w, http.StatusBadRequest, "config too large")
			return
		}
		writeErr(w, http.StatusBadRequest, "invalid body")
		return
	}
	if !templateName.MatchString(req.Name) {
		writeErr(w, http.StatusBadRequest, "name must be 1-63 chars: letters, digits, '.', '-', '_' (no leading separator)")
		return
	}
	// A successful json.RawMessage decode already guarantees config is
	// well-formed JSON; we only need to reject the empty/oversized cases.
	if len(req.Config) == 0 {
		writeErr(w, http.StatusBadRequest, "config is required")
		return
	}
	if len(req.Config) > maxTemplateConfigBytes {
		writeErr(w, http.StatusBadRequest, "config too large")
		return
	}

	t := &templates.Template{User: user.ID, Name: req.Name, Config: req.Config}
	if err := h.Templates.Save(r.Context(), t); err != nil {
		slog.Warn("pool template save failed", "err", err, "user", user.ID, "name", req.Name)
		writeErr(w, http.StatusInternalServerError, "failed to save template")
		return
	}
	writeJSON(w, http.StatusCreated, toPoolTemplateResponse(t))
}

// ListPoolTemplates godoc
//
//	@Summary		List the calling user's pool templates
//	@Description	Returns every pool template owned by the calling user, newest first.
//	@Tags			pool-templates
//	@Produce		json
//	@Success		200	{array}		PoolTemplateResponse
//	@Failure		401	{object}	ErrorResponse
//	@Failure		503	{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/pool-templates [get]
func (h Handlers) ListPoolTemplates(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	if !h.templatesEnabled(w) {
		return
	}

	list, err := h.Templates.List(r.Context(), user.ID)
	if err != nil {
		slog.Warn("pool template list failed", "err", err, "user", user.ID)
		writeErr(w, http.StatusInternalServerError, "failed to list templates")
		return
	}
	out := make([]PoolTemplateResponse, 0, len(list))
	for _, t := range list {
		out = append(out, toPoolTemplateResponse(t))
	}
	writeJSON(w, http.StatusOK, out)
}

// GetPoolTemplate godoc
//
//	@Summary		Get one of the calling user's pool templates
//	@Tags			pool-templates
//	@Produce		json
//	@Param			name	path		string	true	"Template name"
//	@Success		200		{object}	PoolTemplateResponse
//	@Failure		401		{object}	ErrorResponse
//	@Failure		404		{object}	ErrorResponse
//	@Failure		503		{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/pool-templates/{name} [get]
func (h Handlers) GetPoolTemplate(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	if !h.templatesEnabled(w) {
		return
	}

	name := r.PathValue("name")
	if !templateName.MatchString(name) {
		writeErr(w, http.StatusBadRequest, "invalid template name")
		return
	}
	t, err := h.Templates.Get(r.Context(), user.ID, name)
	if err != nil {
		slog.Warn("pool template get failed", "err", err, "user", user.ID, "name", name)
		writeErr(w, http.StatusInternalServerError, "failed to get template")
		return
	}
	if t == nil {
		writeErr(w, http.StatusNotFound, "template not found")
		return
	}
	writeJSON(w, http.StatusOK, toPoolTemplateResponse(t))
}

// DeletePoolTemplate godoc
//
//	@Summary		Delete one of the calling user's pool templates
//	@Tags			pool-templates
//	@Param			name	path	string	true	"Template name"
//	@Success		204
//	@Failure		401	{object}	ErrorResponse
//	@Failure		404	{object}	ErrorResponse
//	@Failure		503	{object}	ErrorResponse
//	@Security		BearerAuth
//	@Router			/api/pool-templates/{name} [delete]
func (h Handlers) DeletePoolTemplate(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	if !h.templatesEnabled(w) {
		return
	}

	name := r.PathValue("name")
	if !templateName.MatchString(name) {
		writeErr(w, http.StatusBadRequest, "invalid template name")
		return
	}
	existed, err := h.Templates.Delete(r.Context(), user.ID, name)
	if err != nil {
		slog.Warn("pool template delete failed", "err", err, "user", user.ID, "name", name)
		writeErr(w, http.StatusInternalServerError, "failed to delete template")
		return
	}
	if !existed {
		writeErr(w, http.StatusNotFound, "template not found")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
