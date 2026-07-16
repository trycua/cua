package handlers

import "net/http"

func (h Handlers) DeprecatedBatchRoute(w http.ResponseWriter, _ *http.Request) {
	writeErr(w, http.StatusGone,
		"/api/batch and /api/label are deprecated and unavailable; use the replacement claim-based flow")
}
