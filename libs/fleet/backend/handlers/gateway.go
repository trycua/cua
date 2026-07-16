package handlers

import (
	"net/http"
)

// Gateway godoc
//
//	@Summary		DEPRECATED: Per-pool orchestrator reverse-proxy (CUA-609)
//	@Description	The per-pool HTTP orchestrator has been deprecated and removed. All pools now use OSGymSandboxClaim CRs exclusively (Path B). This endpoint returns 410 Gone for any request. See CUA-609.
//	@Tags			gateway
//	@Param			name	path		string	true	"Pool name (DNS-1123 label)"
//	@Param			path	path		string	false	"Upstream path (ignored)"
//	@Success		410		{object}	ErrorResponse	"Orchestrator HTTP layer deprecated (CUA-609)"
//	@Router			/api/gateway/{name}/{path} [get]
func (h Handlers) Gateway(w http.ResponseWriter, r *http.Request) {
	// CUA-609: The per-pool orchestrator HTTP layer has been removed.
	// Per-pool orchestrators (Deployments in <pool>-orchestrator namespaces)
	// no longer run. All VM operations go through OSGymSandboxClaim CRs.
	// Zero HTTP traffic was confirmed before removal.
	writeErr(w, http.StatusGone,
		"orchestrator HTTP layer deprecated (CUA-609): use OSGymSandboxClaim CRs instead")
}
