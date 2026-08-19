package handlers

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"strings"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/featureflagadmin"
	"cyclops-cs-backend/middlewares"
	"github.com/trycua/cloud/pkg/featureflags"
)

type CreateFeatureFlagRequest struct {
	Key         string                 `json:"key"`
	ValueType   featureflags.ValueType `json:"value_type"`
	Value       any                    `json:"value"`
	Description string                 `json:"description,omitempty"`
}

type UpdateFeatureFlagRequest struct {
	ValueType       featureflags.ValueType `json:"value_type"`
	Value           any                    `json:"value"`
	ExpectedVersion int64                  `json:"expected_version"`
}

type DeleteFeatureFlagRequest struct {
	ExpectedVersion int64 `json:"expected_version"`
}

type AdminAPIError struct {
	Code    string                 `json:"code"`
	Message string                 `json:"message"`
	Current *featureflagadmin.Flag `json:"current,omitempty"`
}

const maxFeatureFlagRequestBytes = 64 << 10

// ListFeatureFlags godoc
// @Summary List Cyclops feature flags
// @Description Lists direct non-SecureString parameters under /feature-flags/cyclops-cs/ with typed values, ownership, and SSM versions.
// @Tags admin feature flags
// @Produce json
// @Success 200 {array} featureflagadmin.Flag
// @Failure 401 {object} ErrorResponse
// @Failure 403 {object} AdminAPIError
// @Failure 501 {object} AdminAPIError
// @Failure 502 {object} AdminAPIError
// @Security BearerAuth
// @Router /api/admin/feature-flags [get]
func (h Handlers) ListFeatureFlags(w http.ResponseWriter, r *http.Request) {
	if !h.requireFeatureFlagAdmin(w, r) {
		return
	}
	if h.FeatureFlags == nil {
		writeAdminAPIError(w, &featureflagadmin.ServiceError{Code: "unsupported_provider", HTTPStatus: http.StatusNotImplemented, Message: "feature flag management is unavailable"})
		return
	}
	flags, err := h.FeatureFlags.List(r.Context())
	if err != nil {
		writeAdminAPIError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, flags)
}

// CreateFeatureFlag godoc
// @Summary Create an ad hoc Cyclops feature flag
// @Description Creates a String parameter under /feature-flags/cyclops-cs/ with cyclops-cs-admin ownership and a typed logical value.
// @Tags admin feature flags
// @Accept json
// @Produce json
// @Param request body CreateFeatureFlagRequest true "Feature flag to create"
// @Success 201 {object} featureflagadmin.Flag
// @Failure 400 {object} AdminAPIError
// @Failure 401 {object} ErrorResponse
// @Failure 403 {object} AdminAPIError
// @Failure 409 {object} AdminAPIError
// @Failure 422 {object} AdminAPIError
// @Failure 500 {object} AdminAPIError
// @Failure 501 {object} AdminAPIError
// @Failure 502 {object} AdminAPIError
// @Failure 503 {object} AdminAPIError
// @Security BearerAuth
// @Router /api/admin/feature-flags [post]
func (h Handlers) CreateFeatureFlag(w http.ResponseWriter, r *http.Request) {
	if !h.requireFeatureFlagAdmin(w, r) {
		return
	}
	var request CreateFeatureFlagRequest
	if !decodeFeatureFlagJSON(w, r, &request) {
		return
	}
	if h.FeatureFlags == nil {
		writeAdminAPIError(w, &featureflagadmin.ServiceError{Code: "unsupported_provider", HTTPStatus: http.StatusNotImplemented, Message: "feature flag management is unavailable"})
		return
	}
	flag, err := h.FeatureFlags.Create(r.Context(), featureFlagActor(r), featureflagadmin.CreateInput{Key: request.Key, ValueType: request.ValueType, Value: request.Value, Description: request.Description})
	if err != nil {
		writeAdminAPIError(w, err)
		return
	}
	writeJSON(w, http.StatusCreated, flag)
}

// UpdateFeatureFlag godoc
// @Summary Update a Cyclops feature flag value
// @Description Updates the typed value when expected_version matches. Terraform and external ownership protect the key from deletion, not value edits.
// @Tags admin feature flags
// @Accept json
// @Produce json
// @Param key path string true "Flat feature flag key"
// @Param request body UpdateFeatureFlagRequest true "Typed value and expected SSM version"
// @Success 200 {object} featureflagadmin.Flag
// @Failure 400 {object} AdminAPIError
// @Failure 401 {object} ErrorResponse
// @Failure 403 {object} AdminAPIError
// @Failure 404 {object} AdminAPIError
// @Failure 409 {object} AdminAPIError
// @Failure 422 {object} AdminAPIError
// @Failure 500 {object} AdminAPIError
// @Failure 501 {object} AdminAPIError
// @Failure 502 {object} AdminAPIError
// @Failure 503 {object} AdminAPIError
// @Security BearerAuth
// @Router /api/admin/feature-flags/{key} [put]
func (h Handlers) UpdateFeatureFlag(w http.ResponseWriter, r *http.Request) {
	if !h.requireFeatureFlagAdmin(w, r) {
		return
	}
	var request UpdateFeatureFlagRequest
	if !decodeFeatureFlagJSON(w, r, &request) {
		return
	}
	if h.FeatureFlags == nil {
		writeAdminAPIError(w, &featureflagadmin.ServiceError{Code: "unsupported_provider", HTTPStatus: http.StatusNotImplemented, Message: "feature flag management is unavailable"})
		return
	}
	flag, err := h.FeatureFlags.Update(r.Context(), featureFlagActor(r), r.PathValue("key"), featureflagadmin.UpdateInput{ValueType: request.ValueType, Value: request.Value, ExpectedVersion: request.ExpectedVersion})
	if err != nil {
		writeAdminAPIError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, flag)
}

// DeleteFeatureFlag godoc
// @Summary Delete an ad hoc Cyclops feature flag
// @Description Deletes only cyclops-cs-admin-owned flags when expected_version matches. Terraform and external keys are protected.
// @Tags admin feature flags
// @Accept json
// @Produce json
// @Param key path string true "Flat feature flag key"
// @Param request body DeleteFeatureFlagRequest true "Expected SSM version"
// @Success 204
// @Failure 400 {object} AdminAPIError
// @Failure 401 {object} ErrorResponse
// @Failure 403 {object} AdminAPIError
// @Failure 404 {object} AdminAPIError
// @Failure 409 {object} AdminAPIError
// @Failure 422 {object} AdminAPIError
// @Failure 500 {object} AdminAPIError
// @Failure 501 {object} AdminAPIError
// @Failure 502 {object} AdminAPIError
// @Failure 503 {object} AdminAPIError
// @Security BearerAuth
// @Router /api/admin/feature-flags/{key} [delete]
func (h Handlers) DeleteFeatureFlag(w http.ResponseWriter, r *http.Request) {
	if !h.requireFeatureFlagAdmin(w, r) {
		return
	}
	var request DeleteFeatureFlagRequest
	if !decodeFeatureFlagJSON(w, r, &request) {
		return
	}
	if h.FeatureFlags == nil {
		writeAdminAPIError(w, &featureflagadmin.ServiceError{Code: "unsupported_provider", HTTPStatus: http.StatusNotImplemented, Message: "feature flag management is unavailable"})
		return
	}
	if err := h.FeatureFlags.Delete(r.Context(), featureFlagActor(r), r.PathValue("key"), request.ExpectedVersion); err != nil {
		writeAdminAPIError(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

var evalFeatureFlagAdmin = auth.EvalIsAdminFresh

func (h Handlers) requireFeatureFlagAdmin(w http.ResponseWriter, r *http.Request) bool {
	allowed, err := evalFeatureFlagAdmin(r.Context(), currentUser(r))
	if err != nil {
		slog.WarnContext(r.Context(), "opa: feature flag admin eval failed", "err", err)
		if r.Method != http.MethodGet {
			logFeatureFlagAdminRejection(r, "policy_error")
		}
		writeAdminAPIError(w, &featureflagadmin.ServiceError{Code: "policy_error", HTTPStatus: http.StatusInternalServerError, Message: "feature flag authorization evaluation failed"})
		return false
	}
	if allowed {
		return true
	}
	if r.Method != http.MethodGet {
		logFeatureFlagAdminRejection(r, "not_admin")
	}
	writeAdminAPIError(w, &featureflagadmin.ServiceError{Code: "not_admin", HTTPStatus: http.StatusForbidden, Message: "feature flag administration requires an admin user"})
	return false
}

func featureFlagActor(r *http.Request) featureflagadmin.Actor {
	user := currentUser(r)
	if user == nil {
		return featureflagadmin.Actor{}
	}
	traceID, _ := r.Context().Value(middlewares.ContextKey("traceId")).(string)
	return featureflagadmin.Actor{Subject: user.ID, Email: user.Email, PrincipalType: user.PrincipalType, TraceID: traceID}
}

func decodeFeatureFlagJSON(w http.ResponseWriter, r *http.Request, destination any) bool {
	defer r.Body.Close()
	body, err := io.ReadAll(io.LimitReader(r.Body, maxFeatureFlagRequestBytes+1))
	if err != nil || len(body) > maxFeatureFlagRequestBytes {
		logInvalidFeatureFlagRequest(r, "body_too_large")
		writeAdminAPIError(w, &featureflagadmin.ServiceError{Code: "invalid_request", HTTPStatus: http.StatusBadRequest, Message: "request body exceeds 64 KiB"})
		return false
	}
	decoder := json.NewDecoder(bytes.NewReader(body))
	decoder.DisallowUnknownFields()
	decoder.UseNumber()
	if err := decoder.Decode(destination); err != nil {
		parseFailure := "malformed_json"
		if strings.HasPrefix(err.Error(), "json: unknown field ") {
			parseFailure = "unknown_field"
		}
		logInvalidFeatureFlagRequest(r, parseFailure)
		writeAdminAPIError(w, &featureflagadmin.ServiceError{Code: "invalid_request", HTTPStatus: http.StatusBadRequest, Message: "request body must be valid JSON"})
		return false
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		logInvalidFeatureFlagRequest(r, "trailing_data")
		writeAdminAPIError(w, &featureflagadmin.ServiceError{Code: "invalid_request", HTTPStatus: http.StatusBadRequest, Message: "request body must contain one JSON value"})
		return false
	}
	return true
}

func logFeatureFlagAdminRejection(r *http.Request, reason string) {
	actor := featureFlagActor(r)
	key := r.PathValue("key")
	slog.WarnContext(r.Context(), "feature flag mutation rejected by handler authorization",
		"event", "feature_flag_admin",
		"actor", actor.Subject,
		"actor_email", actor.Email,
		"principal_type", actor.PrincipalType,
		"operation", strings.ToLower(r.Method),
		"key", featureflagadmin.BoundedAuditKey(key),
		"path", featureflagadmin.BoundedAuditPath(r.URL.Path, key),
		"traceId", actor.TraceID,
		"result", "rejected",
		"reason", reason,
	)
}

func logInvalidFeatureFlagRequest(r *http.Request, parseFailure string) {
	actor := featureFlagActor(r)
	key := r.PathValue("key")
	slog.WarnContext(r.Context(), "feature flag mutation request rejected",
		"event", "feature_flag_admin",
		"actor", actor.Subject,
		"actor_email", actor.Email,
		"principal_type", actor.PrincipalType,
		"operation", strings.ToLower(r.Method),
		"key", featureflagadmin.BoundedAuditKey(key),
		"path", featureflagadmin.BoundedAuditPath(r.URL.Path, key),
		"traceId", actor.TraceID,
		"result", "rejected",
		"reason", "invalid_request",
		"parse_failure", parseFailure,
	)
}

func writeAdminAPIError(w http.ResponseWriter, err error) {
	var serviceError *featureflagadmin.ServiceError
	if errors.As(err, &serviceError) {
		writeJSON(w, serviceError.HTTPStatus, AdminAPIError{Code: serviceError.Code, Message: serviceError.Message, Current: serviceError.Current})
		return
	}
	writeJSON(w, http.StatusInternalServerError, AdminAPIError{Code: "internal_error", Message: "feature flag operation failed"})
}
