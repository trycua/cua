package handlers

import (
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/productanalytics"
)

const (
	fleetAttributionBodyMaxBytes = 2048
	fleetAttributionTTL          = 7 * 24 * time.Hour
)

var fleetAttributionProperties = map[string]string{
	"campaign_id":  productanalytics.FirstTouchCampaignIDProperty,
	"content_id":   productanalytics.FirstTouchContentIDProperty,
	"utm_source":   productanalytics.FirstTouchUTMSourceProperty,
	"utm_medium":   productanalytics.FirstTouchUTMMediumProperty,
	"utm_campaign": productanalytics.FirstTouchUTMCampaignProperty,
}

type fleetAttributionRecord struct {
	Version    int               `json:"version"`
	CapturedAt int64             `json:"capturedAt"`
	Values     map[string]string `json:"values"`
}

func (h Handlers) RecordFleetAttribution(w http.ResponseWriter, r *http.Request) {
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

	r.Body = http.MaxBytesReader(w, r.Body, fleetAttributionBodyMaxBytes)
	decoder := json.NewDecoder(r.Body)
	decoder.DisallowUnknownFields()
	var record fleetAttributionRecord
	if err := decoder.Decode(&record); err != nil {
		var maxBytesError *http.MaxBytesError
		if errors.As(err, &maxBytesError) {
			writeErr(w, http.StatusRequestEntityTooLarge, "attribution record is too large")
			return
		}
		writeErr(w, http.StatusBadRequest, "invalid attribution record")
		return
	}
	if err := decoder.Decode(&struct{}{}); err != io.EOF {
		writeErr(w, http.StatusBadRequest, "invalid attribution record")
		return
	}

	setOnce, valid := validateFleetAttribution(record, time.Now())
	if !valid {
		writeErr(w, http.StatusBadRequest, "invalid attribution record")
		return
	}
	if productanalytics.ClassifyIdentity(user) != productanalytics.IdentityExternal {
		w.WriteHeader(http.StatusNoContent)
		return
	}

	capturer := h.Analytics
	if capturer == nil {
		capturer = productanalytics.Nop()
	}
	capturer.Capture(productanalytics.Event{
		Name:       productanalytics.EventAttributionBound,
		DistinctID: user.ID,
		Properties: map[string]any{
			"outcome":        productanalytics.OutcomeSuccess,
			"source":         source,
			"principal_type": auth.PrincipalTypeUser,
			"identity_class": productanalytics.IdentityExternal,
		},
		SetOnce: setOnce,
	})
	w.WriteHeader(http.StatusNoContent)
}

func validateFleetAttribution(record fleetAttributionRecord, now time.Time) (map[string]any, bool) {
	if record.Version != 1 || record.CapturedAt <= 0 || len(record.Values) == 0 {
		return nil, false
	}
	age := now.UnixMilli() - record.CapturedAt
	if age < 0 || age >= fleetAttributionTTL.Milliseconds() {
		return nil, false
	}
	setOnce := make(map[string]any, len(record.Values))
	for key, value := range record.Values {
		property, ok := fleetAttributionProperties[key]
		if !ok || !validFleetAttributionValue(value) {
			return nil, false
		}
		setOnce[property] = value
	}
	return setOnce, true
}

func validFleetAttributionValue(value string) bool {
	if value == "" || len(value) > 128 {
		return false
	}
	for _, b := range []byte(value) {
		if !(b >= 'a' && b <= 'z' || b >= 'A' && b <= 'Z' || b >= '0' && b <= '9' || b == '.' || b == '_' || b == '~' || b == '-') {
			return false
		}
	}
	return true
}
