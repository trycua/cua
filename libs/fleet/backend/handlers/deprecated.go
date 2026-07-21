package handlers

import "net/http"

// DeprecatedBatchRoute replies 410 Gone for the retired batch/label API.
// The per-route wrappers below exist only to carry swagger annotations, so
// `swag init` keeps the retired routes documented as deprecated instead of
// silently dropping them from the spec (pinned by
// TestSwaggerMentionsBatchRouteDeprecation).
func (h Handlers) DeprecatedBatchRoute(w http.ResponseWriter, _ *http.Request) {
	writeErr(w, http.StatusGone,
		"/api/batch and /api/label are deprecated and unavailable; use the replacement claim-based flow")
}

// DeprecatedBatchSubmit godoc
//
//	@Summary		Deprecated batch submission route
//	@Description	Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
//	@Deprecated
//	@Tags			batch
//	@Produce		json
//	@Param			pool	path		string	true	"Pool name"
//	@Failure		410		{object}	map[string]string
//	@Security		BearerAuth
//	@Router			/api/batch/{pool}/submit [post]
func (h Handlers) DeprecatedBatchSubmit(w http.ResponseWriter, r *http.Request) {
	h.DeprecatedBatchRoute(w, r)
}

// DeprecatedBatchLanesCreate godoc
//
//	@Summary		Deprecated batch lanes route
//	@Description	Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
//	@Deprecated
//	@Tags			batch
//	@Produce		json
//	@Param			pool	path		string	true	"Pool name"
//	@Failure		410		{object}	map[string]string
//	@Security		BearerAuth
//	@Router			/api/batch/{pool}/lanes [post]
func (h Handlers) DeprecatedBatchLanesCreate(w http.ResponseWriter, r *http.Request) {
	h.DeprecatedBatchRoute(w, r)
}

// DeprecatedBatchLanesDelete godoc
//
//	@Summary		Deprecated batch lanes route
//	@Description	Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
//	@Deprecated
//	@Tags			batch
//	@Produce		json
//	@Param			pool	path		string	true	"Pool name"
//	@Failure		410		{object}	map[string]string
//	@Security		BearerAuth
//	@Router			/api/batch/{pool}/lanes [delete]
func (h Handlers) DeprecatedBatchLanesDelete(w http.ResponseWriter, r *http.Request) {
	h.DeprecatedBatchRoute(w, r)
}

// DeprecatedBatchStatus godoc
//
//	@Summary		Deprecated batch status route
//	@Description	Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
//	@Deprecated
//	@Tags			batch
//	@Produce		json
//	@Param			pool	path		string	true	"Pool name"
//	@Param			id		path		string	true	"Batch ID"
//	@Failure		410		{object}	map[string]string
//	@Security		BearerAuth
//	@Router			/api/batch/{pool}/{id}/status [get]
func (h Handlers) DeprecatedBatchStatus(w http.ResponseWriter, r *http.Request) {
	h.DeprecatedBatchRoute(w, r)
}

// DeprecatedBatchResults godoc
//
//	@Summary		Deprecated batch results route
//	@Description	Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
//	@Deprecated
//	@Tags			batch
//	@Produce		json
//	@Param			pool	path		string	true	"Pool name"
//	@Param			id		path		string	true	"Batch ID"
//	@Failure		410		{object}	map[string]string
//	@Security		BearerAuth
//	@Router			/api/batch/{pool}/{id}/results [get]
func (h Handlers) DeprecatedBatchResults(w http.ResponseWriter, r *http.Request) {
	h.DeprecatedBatchRoute(w, r)
}

// DeprecatedBatchDelete godoc
//
//	@Summary		Deprecated batch delete route
//	@Description	Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
//	@Deprecated
//	@Tags			batch
//	@Produce		json
//	@Param			pool	path		string	true	"Pool name"
//	@Param			id		path		string	true	"Batch ID"
//	@Failure		410		{object}	map[string]string
//	@Security		BearerAuth
//	@Router			/api/batch/{pool}/{id} [delete]
func (h Handlers) DeprecatedBatchDelete(w http.ResponseWriter, r *http.Request) {
	h.DeprecatedBatchRoute(w, r)
}

// DeprecatedLabelBatch godoc
//
//	@Summary		Deprecated label batch route
//	@Description	Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
//	@Deprecated
//	@Tags			label
//	@Produce		json
//	@Param			pool	path		string	true	"Pool name"
//	@Param			label	path		string	true	"Label"
//	@Failure		410		{object}	map[string]string
//	@Security		BearerAuth
//	@Router			/api/label/{pool}/{label}/batch [post]
func (h Handlers) DeprecatedLabelBatch(w http.ResponseWriter, r *http.Request) {
	h.DeprecatedBatchRoute(w, r)
}

// DeprecatedLabelStatus godoc
//
//	@Summary		Deprecated label status route
//	@Description	Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
//	@Deprecated
//	@Tags			label
//	@Produce		json
//	@Param			pool	path		string	true	"Pool name"
//	@Param			label	path		string	true	"Label"
//	@Failure		410		{object}	map[string]string
//	@Security		BearerAuth
//	@Router			/api/label/{pool}/{label}/status [get]
func (h Handlers) DeprecatedLabelStatus(w http.ResponseWriter, r *http.Request) {
	h.DeprecatedBatchRoute(w, r)
}

// DeprecatedLabelResults godoc
//
//	@Summary		Deprecated label results route
//	@Description	Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
//	@Deprecated
//	@Tags			label
//	@Produce		json
//	@Param			pool	path		string	true	"Pool name"
//	@Param			label	path		string	true	"Label"
//	@Failure		410		{object}	map[string]string
//	@Security		BearerAuth
//	@Router			/api/label/{pool}/{label}/results [get]
func (h Handlers) DeprecatedLabelResults(w http.ResponseWriter, r *http.Request) {
	h.DeprecatedBatchRoute(w, r)
}

// DeprecatedLabelDelete godoc
//
//	@Summary		Deprecated label delete route
//	@Description	Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
//	@Deprecated
//	@Tags			label
//	@Produce		json
//	@Param			pool	path		string	true	"Pool name"
//	@Param			label	path		string	true	"Label"
//	@Failure		410		{object}	map[string]string
//	@Security		BearerAuth
//	@Router			/api/label/{pool}/{label} [delete]
func (h Handlers) DeprecatedLabelDelete(w http.ResponseWriter, r *http.Request) {
	h.DeprecatedBatchRoute(w, r)
}
