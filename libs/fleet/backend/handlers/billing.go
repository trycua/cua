package handlers

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"strconv"
	"strings"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/billing"
	"cyclops-cs-backend/middlewares"
	"cyclops-cs-backend/productanalytics"
)

type BillingService interface {
	AttachedCards(ctx context.Context, subject string) ([]billing.SavedCard, error)
	Summary(ctx context.Context, subject string) (billing.Summary, error)
	CreateSetupSession(ctx context.Context, subject string, options billing.SetupOptions) (string, error)
	CreatePortalSession(ctx context.Context, subject, returnURL string) (string, error)
	SetDefaultPaymentMethodForSetupGeneration(ctx context.Context, customerID, paymentMethodID, generation string) (bool, error)
}

type BillingUsageService interface {
	Usage(ctx context.Context, subject string, months int, now time.Time) (billing.Usage, error)
}

type BillingSetupCompletionService interface {
	CompleteSetupSession(ctx context.Context, subject, source, identityClass, sessionID string) (billing.SetupCompletion, error)
}

type BillingSessionResponse struct {
	URL string `json:"url"`
}

type BillingSetupCompletionRequest struct {
	SessionID string `json:"session_id"`
}

type BillingSetupCompletionResponse struct {
	Applied bool `json:"applied"`
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

func decodeBillingSetupCompletion(w http.ResponseWriter, r *http.Request) (BillingSetupCompletionRequest, bool) {
	r.Body = http.MaxBytesReader(w, r.Body, 512)
	decoder := json.NewDecoder(r.Body)
	decoder.DisallowUnknownFields()
	var request BillingSetupCompletionRequest
	if err := decoder.Decode(&request); err != nil {
		var maxErr *http.MaxBytesError
		if errors.As(err, &maxErr) {
			writeErr(w, http.StatusRequestEntityTooLarge, "billing setup completion request is too large")
			return BillingSetupCompletionRequest{}, false
		}
		writeErr(w, http.StatusBadRequest, "invalid billing setup completion request")
		return BillingSetupCompletionRequest{}, false
	}
	if err := decoder.Decode(&struct{}{}); err != io.EOF {
		writeErr(w, http.StatusBadRequest, "invalid billing setup completion request")
		return BillingSetupCompletionRequest{}, false
	}
	request.SessionID = strings.TrimSpace(request.SessionID)
	if !validCheckoutSessionID(request.SessionID) {
		writeErr(w, http.StatusBadRequest, "invalid billing setup completion request")
		return BillingSetupCompletionRequest{}, false
	}
	return request, true
}

func validCheckoutSessionID(sessionID string) bool {
	if len(sessionID) < len("cs_x") || len(sessionID) > 255 || !strings.HasPrefix(sessionID, "cs_") {
		return false
	}
	for _, char := range sessionID {
		if (char < 'a' || char > 'z') && (char < 'A' || char > 'Z') && (char < '0' || char > '9') && char != '_' {
			return false
		}
	}
	return true
}

// poolCreateCardRequired resolves the advisory pool_create_card_required
// summary field from the same admission policy /api/k8s enforces. The field
// only pre-warns the dashboard — the policy still denies bypassing creates —
// so evaluation failures fail open to not-required instead of erroring the
// whole summary.
func (h Handlers) poolCreateCardRequired(r *http.Request, user *auth.User) bool {
	required, err := auth.EvalPoolCreateCardRequired(r.Context(), user, func(ctx context.Context) (auth.FactSet, error) {
		return StripeCardFacts(h).LoadFacts(ctx, r)
	})
	if err != nil {
		slog.WarnContext(r.Context(), "billing: card requirement eval failed; reporting not required", "err", err)
		return false
	}
	return required
}

// GetBillingSummary godoc
// @Summary Billing summary
// @Description Returns a sanitized Stripe-backed billing summary for the authenticated Cyclops subject, including whether creating pools currently requires adding a payment card.
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
	summary.PoolCreateCardRequired = h.poolCreateCardRequired(r, user)
	writeJSON(w, http.StatusOK, summary)
}

// GetBillingUsage godoc
// @Summary Billing usage
// @Description Returns Stripe-backed invoice spend, trend, and current-period line-item breakdown for the authenticated Fleet subject.
// @Tags billing
// @Produce json
// @Param months query int false "History window in months" Enums(3, 6, 12) default(6)
// @Success 200 {object} billing.Usage
// @Failure 400 {object} map[string]string
// @Failure 401 {object} map[string]string
// @Failure 403 {object} map[string]string
// @Failure 502 {object} map[string]string
// @Failure 503 {object} map[string]string
// @Security BearerAuth
// @Router /api/billing/usage [get]
func (h Handlers) GetBillingUsage(w http.ResponseWriter, r *http.Request) {
	user, ok := requireBillingEnabled(w, r)
	if !ok || !h.billingAvailable(w) {
		return
	}
	months := 6
	if raw := r.URL.Query().Get("months"); raw != "" {
		parsed, err := strconv.Atoi(raw)
		if err != nil || (parsed != 3 && parsed != 6 && parsed != 12) {
			writeErr(w, http.StatusBadRequest, "months must be 3, 6, or 12")
			return
		}
		months = parsed
	}
	usageService, ok := h.Billing.(BillingUsageService)
	if !ok {
		writeErr(w, http.StatusServiceUnavailable, "billing usage is not configured")
		return
	}
	usage, err := usageService.Usage(r.Context(), user.ID, months, time.Now())
	if err != nil {
		writeErr(w, http.StatusBadGateway, "could not load billing usage")
		return
	}
	writeJSON(w, http.StatusOK, usage)
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
	if h.Stripe.CheckoutSuccessURL == "" || h.Stripe.CheckoutCancelURL == "" ||
		strings.Count(h.Stripe.CheckoutSuccessURL, "{CHECKOUT_SESSION_ID}") != 1 {
		writeErr(w, http.StatusServiceUnavailable, "Stripe card setup is not configured; a session-aware success URL and cancel URL are required")
		return
	}
	source, sourceOK := productanalytics.SourceForUser(user, h.AuthCfg.SPAClientID)
	if !sourceOK || source != productanalytics.SourceSPA {
		writeErr(w, http.StatusForbidden, "Fleet browser session is required")
		return
	}
	captureSetupStart := func(outcome string, statusCode int, errorClass string) {
		traceID, _ := r.Context().Value(middlewares.ContextKey("traceId")).(string)
		capturer := h.Analytics
		if capturer == nil {
			capturer = productanalytics.Nop()
		}
		properties := map[string]any{
			"outcome":        outcome,
			"source":         source,
			"principal_type": user.PrincipalType,
			"identity_class": productanalytics.ClassifyIdentity(user),
			"status_code":    statusCode,
		}
		if errorClass != "" {
			properties["error_class"] = errorClass
		}
		capturer.Capture(productanalytics.Event{
			Name: productanalytics.EventPaymentSetupStart, DistinctID: user.ID, InsertID: traceID,
			Properties: properties,
		})
	}
	url, err := h.Billing.CreateSetupSession(r.Context(), user.ID, billing.SetupOptions{
		SuccessURL:    h.Stripe.CheckoutSuccessURL,
		CancelURL:     h.Stripe.CheckoutCancelURL,
		Source:        source,
		IdentityClass: string(productanalytics.ClassifyIdentity(user)),
	})
	if err != nil {
		captureSetupStart(productanalytics.OutcomeFailure, http.StatusBadGateway, "payment_provider")
		writeErr(w, http.StatusBadGateway, "could not create Stripe card setup Session")
		return
	}
	captureSetupStart(productanalytics.OutcomeSuccess, http.StatusOK, "")
	writeJSON(w, http.StatusOK, BillingSessionResponse{URL: url})
}

// CompleteBillingSetupSession godoc
// @Summary Complete Stripe card setup Session
// @Description Reconciles an authenticated subject's completed Stripe Checkout Session and configures its card as the default payment method.
// @Tags billing
// @Accept json
// @Produce json
// @Param request body BillingSetupCompletionRequest true "Completed Checkout Session"
// @Success 200 {object} BillingSetupCompletionResponse
// @Failure 400 {object} map[string]string
// @Failure 401 {object} map[string]string
// @Failure 403 {object} map[string]string
// @Failure 404 {object} map[string]string
// @Failure 409 {object} map[string]string
// @Failure 502 {object} map[string]string
// @Failure 503 {object} map[string]string
// @Security BearerAuth
// @Router /api/billing/setup-session/complete [post]
func (h Handlers) CompleteBillingSetupSession(w http.ResponseWriter, r *http.Request) {
	user, ok := requireBillingEnabled(w, r)
	if !ok || !h.billingAvailable(w) {
		return
	}
	source, sourceOK := productanalytics.SourceForUser(user, h.AuthCfg.SPAClientID)
	if !sourceOK || source != productanalytics.SourceSPA {
		writeErr(w, http.StatusForbidden, "Fleet browser session is required")
		return
	}
	request, ok := decodeBillingSetupCompletion(w, r)
	if !ok {
		return
	}
	completionService, ok := h.Billing.(BillingSetupCompletionService)
	if !ok {
		writeErr(w, http.StatusServiceUnavailable, "billing setup completion is not configured")
		return
	}
	result, err := completionService.CompleteSetupSession(
		r.Context(), user.ID, source, string(productanalytics.ClassifyIdentity(user)), request.SessionID,
	)
	switch {
	case errors.Is(err, billing.ErrSetupSessionNotFound), errors.Is(err, billing.ErrSetupSessionNotOwned):
		writeErr(w, http.StatusNotFound, "billing setup session was not found")
		return
	case errors.Is(err, billing.ErrSetupSessionIncomplete):
		writeErr(w, http.StatusConflict, "Stripe card setup is not complete")
		return
	case errors.Is(err, billing.ErrSetupSessionInvalid):
		writeErr(w, http.StatusBadRequest, "invalid Stripe card setup session")
		return
	case err != nil:
		writeErr(w, http.StatusBadGateway, "could not complete Stripe card setup")
		return
	}
	if result.Applied {
		capturer := h.Analytics
		if capturer == nil {
			capturer = productanalytics.Nop()
		}
		capturer.Capture(productanalytics.Event{
			Name: productanalytics.EventPaymentMethodSetup, DistinctID: user.ID,
			InsertID: "fleet-payment-setup:" + result.SetupIntentID,
			Properties: map[string]any{
				"outcome":        productanalytics.OutcomeSuccess,
				"source":         source,
				"principal_type": user.PrincipalType,
				"identity_class": productanalytics.ClassifyIdentity(user),
			},
		})
	}
	writeJSON(w, http.StatusOK, BillingSetupCompletionResponse{Applied: result.Applied})
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
