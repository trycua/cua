package handlers

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"strings"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/billing"
)

type BillingService interface {
	Summary(ctx context.Context, subject string) (billing.Summary, error)
	CreateSetupSession(ctx context.Context, subject string, options billing.SetupOptions) (string, error)
	CreatePortalSession(ctx context.Context, subject, returnURL string) (string, error)
	SetDefaultPaymentMethodForSetupGeneration(ctx context.Context, customerID, paymentMethodID, generation string) (bool, error)
}

type BillingSessionResponse struct {
	URL string `json:"url"`
}

func (h Handlers) billingAvailable(w http.ResponseWriter) bool {
	if h.Billing == nil || h.Stripe.SecretKey == "" {
		writeErr(w, http.StatusServiceUnavailable, "Stripe billing is not configured")
		return false
	}
	return true
}

func requireBillingEnabled(w http.ResponseWriter, r *http.Request) (*auth.User, bool) {
	user := currentUser(r)
	if user == nil || user.ID == "" {
		writeErr(w, http.StatusUnauthorized, "authenticated user is required")
		return nil, false
	}

	enabled, err := auth.EvalBillingEnabled(r.Context(), user)
	if err != nil {
		slog.WarnContext(r.Context(), "billing flag eval failed; denying request", "err", err)
	}
	if !enabled {
		writeErr(w, http.StatusForbidden, "billing is disabled")
		return nil, false
	}
	return user, true
}

func requireEmptyBillingBody(w http.ResponseWriter, r *http.Request) bool {
	if r.Body == nil {
		return true
	}
	r.Body = http.MaxBytesReader(w, r.Body, 1024)
	body, err := io.ReadAll(r.Body)
	if err != nil {
		var maxErr *http.MaxBytesError
		if errors.As(err, &maxErr) {
			writeErr(w, http.StatusRequestEntityTooLarge, "billing request body is too large")
		} else {
			writeErr(w, http.StatusBadRequest, "could not read billing request")
		}
		return false
	}
	if strings.TrimSpace(string(body)) != "" {
		writeErr(w, http.StatusBadRequest, "billing request body must be empty")
		return false
	}
	return true
}

// GetBillingSummary godoc
// @Summary Billing summary
// @Description Returns a sanitized Stripe-backed billing summary for the authenticated Cyclops subject.
// @Tags billing
// @Produce json
// @Success 200 {object} billing.Summary
// @Failure 401 {object} map[string]string
// @Failure 403 {object} map[string]string
// @Failure 503 {object} map[string]string
// @Security BearerAuth
// @Router /api/billing/summary [get]
func (h Handlers) GetBillingSummary(w http.ResponseWriter, r *http.Request) {
	user, ok := requireBillingEnabled(w, r)
	if !ok {
		return
	}
	if !h.billingAvailable(w) {
		return
	}
	summary, err := h.Billing.Summary(r.Context(), user.ID)
	if err != nil {
		writeErr(w, http.StatusBadGateway, "could not load billing summary")
		return
	}
	writeJSON(w, http.StatusOK, summary)
}

// CreateBillingSetupSession godoc
// @Summary Create Stripe card setup Session
// @Description Creates a Stripe-hosted Checkout Session in setup mode for reusable off-session card collection.
// @Tags billing
// @Produce json
// @Success 200 {object} BillingSessionResponse
// @Failure 400 {object} map[string]string
// @Failure 401 {object} map[string]string
// @Failure 403 {object} map[string]string
// @Failure 503 {object} map[string]string
// @Security BearerAuth
// @Router /api/billing/setup-session [post]
func (h Handlers) CreateBillingSetupSession(w http.ResponseWriter, r *http.Request) {
	user, ok := requireBillingEnabled(w, r)
	if !ok || !requireEmptyBillingBody(w, r) || !h.billingAvailable(w) {
		return
	}
	if h.Stripe.CheckoutSuccessURL == "" || h.Stripe.CheckoutCancelURL == "" {
		writeErr(w, http.StatusServiceUnavailable, "Stripe card setup is not configured; redirect URLs are required")
		return
	}
	url, err := h.Billing.CreateSetupSession(r.Context(), user.ID, billing.SetupOptions{
		SuccessURL: h.Stripe.CheckoutSuccessURL,
		CancelURL:  h.Stripe.CheckoutCancelURL,
	})
	if err != nil {
		writeErr(w, http.StatusBadGateway, "could not create Stripe card setup Session")
		return
	}
	writeJSON(w, http.StatusOK, BillingSessionResponse{URL: url})
}

// CreateBillingPortalSession godoc
// @Summary Create Stripe Billing Portal Session
// @Description Creates a Stripe-hosted Billing Portal Session for the customer owned by the authenticated Cyclops subject.
// @Tags billing
// @Produce json
// @Success 200 {object} BillingSessionResponse
// @Failure 400 {object} map[string]string
// @Failure 401 {object} map[string]string
// @Failure 403 {object} map[string]string
// @Failure 404 {object} map[string]string
// @Failure 503 {object} map[string]string
// @Security BearerAuth
// @Router /api/billing/portal-session [post]
func (h Handlers) CreateBillingPortalSession(w http.ResponseWriter, r *http.Request) {
	user, ok := requireBillingEnabled(w, r)
	if !ok {
		return
	}
	if !requireEmptyBillingBody(w, r) {
		return
	}
	if !h.billingAvailable(w) {
		return
	}
	if h.Stripe.PortalReturnURL == "" {
		writeErr(w, http.StatusServiceUnavailable, "Stripe Billing Portal is not configured; STRIPE_PORTAL_RETURN_URL is required")
		return
	}
	url, err := h.Billing.CreatePortalSession(r.Context(), user.ID, h.Stripe.PortalReturnURL)
	if errors.Is(err, billing.ErrCustomerNotFound) {
		writeErr(w, http.StatusNotFound, "billing customer was not found")
		return
	}
	if err != nil {
		writeErr(w, http.StatusBadGateway, "could not create Stripe Billing Portal Session")
		return
	}
	writeJSON(w, http.StatusOK, BillingSessionResponse{URL: url})
}
