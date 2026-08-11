package handlers

import (
	"net/http"
	"sync/atomic"
)

// HealthResponse is the body of GET /healthz and GET /readyz.
type HealthResponse struct {
	OK bool `json:"ok"`
}

// Readiness tracks whether startup dependencies required for serving traffic
// have completed successfully. It starts fail-closed.
type Readiness struct {
	databaseReady atomic.Bool
}

func NewReadiness() *Readiness {
	return &Readiness{}
}

func (r *Readiness) MarkReady() {
	r.databaseReady.Store(true)
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
	if h.Readiness == nil || !h.Readiness.databaseReady.Load() {
		writeJSON(w, http.StatusServiceUnavailable, HealthResponse{OK: false})
		return
	}
	writeJSON(w, http.StatusOK, HealthResponse{OK: true})
}
