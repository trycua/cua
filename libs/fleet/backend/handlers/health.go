package handlers

import (
	"net/http"
)

// HealthResponse is the body of GET /healthz and GET /readyz.
type HealthResponse struct {
	OK bool `json:"ok"`
}

// GetHealth godoc
//
//	@Summary	Liveness probe
//	@Tags		health
//	@Produce	json
//	@Success	200	{object}	HealthResponse
//	@Router		/healthz [get]
func (h Handlers) GetHealth(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, http.StatusOK, HealthResponse{OK: true})
}

func (h Handlers) GetReadiness(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, http.StatusOK, HealthResponse{OK: true})
}
