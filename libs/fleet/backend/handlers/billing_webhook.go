package handlers

import (
	"errors"
	"io"
	"net/http"

	"cyclops-cs-backend/billing"
	"cyclops-cs-backend/metrics"
)

const stripeWebhookBodyLimit = 64 << 10

type WebhookVerifier interface {
	Verify(payload []byte, signature, secret string) (billing.WebhookEvent, error)
}

// HandleBillingWebhook godoc
// @Summary Receive Stripe webhook
// @Description Verifies a Stripe-signed raw webhook body and configures the default payment method for completed fleet setup intents.
// @Tags billing
// @Accept json
// @Success 204
// @Failure 400 {object} map[string]string
// @Failure 413 {object} map[string]string
// @Failure 502 {object} map[string]string
// @Failure 503 {object} map[string]string
// @Router /api/billing/webhook [post]
func (h Handlers) HandleBillingWebhook(w http.ResponseWriter, r *http.Request) {
	if h.Stripe.WebhookSecret == "" || h.WebhookVerifier == nil {
		metrics.RecordBillingWebhook("unavailable", "")
		writeErr(w, http.StatusServiceUnavailable, "Stripe webhook verification is not configured")
		return
	}

	r.Body = http.MaxBytesReader(w, r.Body, stripeWebhookBodyLimit)
	payload, err := io.ReadAll(r.Body)
	if err != nil {
		var maxErr *http.MaxBytesError
		if errors.As(err, &maxErr) {
			metrics.RecordBillingWebhook("oversized", "")
			writeErr(w, http.StatusRequestEntityTooLarge, "Stripe webhook body is too large")
			return
		}
		metrics.RecordBillingWebhook("read_error", "")
		writeErr(w, http.StatusBadRequest, "could not read Stripe webhook body")
		return
	}

	event, err := h.WebhookVerifier.Verify(payload, r.Header.Get("Stripe-Signature"), h.Stripe.WebhookSecret)
	if err != nil {
		metrics.RecordBillingWebhook("invalid_signature", "")
		writeErr(w, http.StatusBadRequest, "invalid Stripe webhook signature")
		return
	}

	if event.Type != "setup_intent.succeeded" || event.Purpose != billing.SetupPurpose {
		metrics.RecordBillingWebhook("ignored", event.Type)
		w.WriteHeader(http.StatusNoContent)
		return
	}
	if event.CustomerID == "" || event.PaymentMethodID == "" || event.SetupGeneration == "" {
		metrics.RecordBillingWebhook("malformed", event.Type)
		w.WriteHeader(http.StatusNoContent)
		return
	}
	if h.Billing == nil {
		metrics.RecordBillingWebhook("unavailable", event.Type)
		writeErr(w, http.StatusServiceUnavailable, "Stripe billing is not configured")
		return
	}
	applied, err := h.Billing.SetDefaultPaymentMethodForSetupGeneration(r.Context(), event.CustomerID, event.PaymentMethodID, event.SetupGeneration)
	if err != nil {
		metrics.RecordBillingWebhook("update_failed", event.Type)
		writeErr(w, http.StatusBadGateway, "could not configure default payment method")
		return
	}
	if !applied {
		metrics.RecordBillingWebhook("ignored", event.Type)
		w.WriteHeader(http.StatusNoContent)
		return
	}
	metrics.RecordBillingWebhook("configured", event.Type)
	w.WriteHeader(http.StatusNoContent)
}
