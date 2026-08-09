package handlers

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/identity"
	"cyclops-cs-backend/statequery"
)

type StateQueryExecutor interface {
	Execute(context.Context, string, bool, statequery.ValidatedQuery) (statequery.ResultSet, error)
}

type stateQueryRequest struct {
	SQL string `json:"sql"`
}

func (h Handlers) QueryState(w http.ResponseWriter, r *http.Request) {
	if h.StateQueryExecutor == nil {
		writeErr(w, http.StatusServiceUnavailable, "state query unavailable")
		return
	}
	user := currentUser(r)
	if user == nil || user.ID == "" {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}

	limits := statequery.DefaultLimits()
	if h.StateQuery.MaxRows > 0 {
		limits.MaxRows = h.StateQuery.MaxRows
	}
	if h.StateQuery.TimeoutMS > 0 {
		limits.TimeoutMS = h.StateQuery.TimeoutMS
	}
	if h.StateQuery.MaxSQLBytes > 0 {
		limits.MaxSQLBytes = h.StateQuery.MaxSQLBytes
	}
	decoder := json.NewDecoder(http.MaxBytesReader(w, r.Body, int64(limits.MaxSQLBytes+1024)))
	decoder.DisallowUnknownFields()
	var request stateQueryRequest
	if err := decoder.Decode(&request); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid request")
		return
	}
	validated, err := statequery.Validate(request.SQL, limits)
	if err != nil {
		writeErr(w, http.StatusBadRequest, "invalid state query")
		return
	}

	authorize := h.StateQueryAuthorize
	if authorize == nil {
		authorize = auth.EvalIsAdmin
	}
	admin, err := authorize(r.Context(), user)
	if err != nil {
		writeErr(w, http.StatusForbidden, "state query denied")
		return
	}
	result, err := h.StateQueryExecutor.Execute(
		r.Context(), identity.PersonalGroup(r.Context(), user.ID), admin, validated,
	)
	if err != nil {
		if errors.Is(r.Context().Err(), context.DeadlineExceeded) {
			writeErr(w, http.StatusGatewayTimeout, "state query timed out")
			return
		}
		writeErr(w, http.StatusServiceUnavailable, "state query failed")
		return
	}
	writeJSON(w, http.StatusOK, result)
}
