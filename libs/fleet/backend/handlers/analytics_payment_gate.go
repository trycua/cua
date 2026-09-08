package handlers

import (
	"encoding/json"
	"errors"
	"io"
	"net/http"

	"cyclops-cs-backend/middlewares"
	"cyclops-cs-backend/productanalytics"
	"go.opentelemetry.io/otel/trace"
)

const fleetPaymentGateBodyMaxBytes = 256

type fleetPaymentGateRecord struct {
	Reason string `json:"reason"`
}

func (h Handlers) RecordFleetPaymentGate(w http.ResponseWriter, r *http.Request) {
	// The browser reports which bounded gate it rendered. The backend owns the
	// authenticated identity fields; browser-side dedup is best-effort.
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

	r.Body = http.MaxBytesReader(w, r.Body, fleetPaymentGateBodyMaxBytes)
	decoder := json.NewDecoder(r.Body)
	decoder.DisallowUnknownFields()
	var record fleetPaymentGateRecord
	if err := decoder.Decode(&record); err != nil {
		var maxBytesError *http.MaxBytesError
		if errors.As(err, &maxBytesError) {
			writeErr(w, http.StatusRequestEntityTooLarge, "payment gate record is too large")
			return
		}
		writeErr(w, http.StatusBadRequest, "invalid payment gate record")
		return
	}
	if err := decoder.Decode(&struct{}{}); err != io.EOF {
		writeErr(w, http.StatusBadRequest, "invalid payment gate record")
		return
	}
	if record.Reason != productanalytics.ReasonNoPaymentMethod && record.Reason != productanalytics.ReasonCardAdmissionRequired {
		writeErr(w, http.StatusBadRequest, "invalid payment gate record")
		return
	}
	if productanalytics.ClassifyIdentity(user) != productanalytics.IdentityExternal {
		w.WriteHeader(http.StatusNoContent)
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
		Name: productanalytics.EventPaymentGateShown, DistinctID: user.ID, InsertID: traceID,
		Properties: map[string]any{
			"outcome":        productanalytics.OutcomeSuccess,
			"source":         source,
			"principal_type": user.PrincipalType,
			"identity_class": productanalytics.IdentityExternal,
			"resource_type":  "pool",
			"reason":         record.Reason,
		},
	})
	w.WriteHeader(http.StatusNoContent)
}
