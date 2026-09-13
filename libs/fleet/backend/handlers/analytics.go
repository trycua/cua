package handlers

import (
	"net/http"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/middlewares"
	"cyclops-cs-backend/productanalytics"
	"go.opentelemetry.io/otel/trace"
)

func (h Handlers) RecordAnalyticsSession(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil || user.ID == "" {
		writeErr(w, http.StatusUnauthorized, "authenticated user is required")
		return
	}
	source, ok := productanalytics.SourceForUser(user, h.AuthCfg.SPAClientID)
	if !ok || source != productanalytics.SourceSPA {
		writeErr(w, http.StatusForbidden, "Fleet browser session is required")
		return
	}
	traceID := trace.SpanContextFromContext(r.Context()).TraceID().String()
	if traceID == "00000000000000000000000000000000" {
		traceID, _ = r.Context().Value(middlewares.ContextKey("traceId")).(string)
	}
	capturer := h.Analytics
	if capturer == nil {
		capturer = productanalytics.Nop()
	}
	capturer.Capture(productanalytics.Event{
		Name: productanalytics.EventLoginSucceeded, DistinctID: user.ID, InsertID: traceID,
		Properties: map[string]any{
			"outcome":        productanalytics.OutcomeSuccess,
			"source":         source,
			"principal_type": auth.PrincipalTypeUser,
			"identity_class": productanalytics.ClassifyIdentity(user),
		},
	})
	w.WriteHeader(http.StatusNoContent)
}
