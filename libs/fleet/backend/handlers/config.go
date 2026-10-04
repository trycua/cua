package handlers

import (
	"log/slog"
	"net/http"

	"cyclops-cs-backend/auth"
)

// ConfigResponse is the payload returned by GET /api/config.
// The frontend uses this to select the correct UI strategy (admin vs customer).
type UsagePricingConfig struct {
	VCPUHourUSD      float64 `json:"vcpu_hour_usd"`
	MemoryGiBHourUSD float64 `json:"memory_gib_hour_usd"`
}

type ConfigResponse struct {
	// Admin is true when the caller is in input.flags.admin_subs (OPA-evaluated).
	// Non-admins get the customer view: infra-only nav (Nodes, Operator events)
	// is hidden in the SPA and the corresponding kubectl-proxy paths are denied
	// server-side by authz.rego.
	Admin        bool               `json:"admin"`
	Billing      bool               `json:"billing"`
	Chat         bool               `json:"chat"`
	Usage        bool               `json:"usage"`
	UsagePricing UsagePricingConfig `json:"usage_pricing"`
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

	isAdmin, err := h.isAdmin(ctx, user)
	if err != nil {
		// Fail closed to the most restrictive view, and log so a
		// misconfigured OPA store / provider is visible to operators.
		slog.WarnContext(ctx, "opa: is_admin eval failed; defaulting to non-admin", "err", err)
		isAdmin = false
	}

	billingEnabled, err := auth.EvalBillingEnabled(ctx, user)
	if err != nil {
		slog.WarnContext(ctx, "billing flag eval failed; defaulting off", "err", err)
		billingEnabled = false
	}

	pricing, err := h.usagePricing(ctx, user)
	if err != nil {
		slog.WarnContext(ctx, "usage pricing flag eval failed; using defaults for invalid values", "err", err)
	}

	chatEnabled, err := h.chatEnabled(ctx, user)
	if err != nil {
		slog.WarnContext(ctx, "chat access eval failed; defaulting off", "err", err)
		chatEnabled = false
	}
	chatEnabled = chatEnabled && h.Conversations != nil && h.Model != nil

	writeJSON(w, http.StatusOK, ConfigResponse{
		Admin: isAdmin, Billing: billingEnabled, Chat: chatEnabled, Usage: h.Usage != nil,
		UsagePricing: UsagePricingConfig{VCPUHourUSD: pricing.VCPUHourUSD, MemoryGiBHourUSD: pricing.MemoryGiBHourUSD},
	})
}
