package handlers

import (
	"log/slog"
	"net/http"

	"cyclops-cs-backend/auth"
)

// ConfigResponse is the payload returned by GET /api/config.
// The frontend uses this to select the correct UI strategy (admin vs customer).
type ConfigResponse struct {
	// Admin is true when the caller is in input.flags.admin_subs (OPA-evaluated).
	// Non-admins get the customer view: infra-only nav (Nodes, Operator events)
	// is hidden in the SPA and the corresponding kubectl-proxy paths are denied
	// server-side by authz.rego.
	Admin bool `json:"admin"`
}

// GetConfig returns per-user feature flags evaluated by OPA.
//
// @Summary		Per-user feature flags
// @Description	Returns OPA-evaluated feature flags for the authenticated SPA user. `admin` is true when the caller's JWT sub appears in input.flags.admin_subs.
// @Tags			config
// @Produce		json
// @Success		200	{object}	ConfigResponse
// @Failure		401	{object}	ErrorResponse
// @Security		BearerAuth
// @Router			/api/config [get]
func (h Handlers) GetConfig(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	ctx := r.Context()

	isAdmin, err := auth.EvalIsAdmin(ctx, user)
	if err != nil {
		// Fail closed to the most restrictive view, and log so a
		// misconfigured OPA store / provider is visible to operators.
		slog.WarnContext(ctx, "opa: is_admin eval failed; defaulting to non-admin", "err", err)
		isAdmin = false
	}

	writeJSON(w, http.StatusOK, ConfigResponse{Admin: isAdmin})
}
