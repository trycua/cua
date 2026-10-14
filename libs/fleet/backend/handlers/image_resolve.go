package handlers

import (
	"context"
	"errors"
	"net/http"

	"cyclops-cs-backend/imageresolve"

	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
)

// ImageResolver pins an image ref (optionally mapped to a runtime's variant)
// to a digest. imageresolve.Resolver is the production implementation.
type ImageResolver interface {
	Resolve(ctx context.Context, ref, runtime string) (imageresolve.Result, error)
}

// ResolveImage serves GET /api/images/resolve?ref=<ref>[&runtime=<runtime>].
//
// It pins public registry refs server-side so an SDK can create a
// digest-pinned pool without registry access of its own, and maps the
// canonical cua images to the variant the runtime runs (runtime kubevirt:
// ghcr.io/trycua/linux:24.04 -> :24.04-disk). Private refs fail with 422:
// the SDK holds those credentials and resolves them itself.
func (h Handlers) ResolveImage(w http.ResponseWriter, r *http.Request) {
	ctx, span := handlerTracer().Start(r.Context(), "images.resolve")
	defer span.End()
	w.Header().Set("Cache-Control", "private, no-store")

	query := r.URL.Query()
	ref := query.Get("ref")
	runtime := query.Get("runtime")
	if ref == "" {
		writeErr(w, http.StatusBadRequest, "ref query parameter is required")
		return
	}
	if h.ImageResolver == nil {
		writeErr(w, http.StatusServiceUnavailable, "image resolution is not configured")
		return
	}
	span.SetAttributes(attribute.String("images.runtime", runtime))

	result, err := h.ImageResolver.Resolve(ctx, ref, runtime)
	if err != nil {
		status := http.StatusBadGateway
		switch {
		case errors.Is(err, imageresolve.ErrInvalidReference), errors.Is(err, imageresolve.ErrUnsupported):
			status = http.StatusBadRequest
		case errors.Is(err, imageresolve.ErrNotFound):
			status = http.StatusNotFound
		case errors.Is(err, imageresolve.ErrRegistryNotAllowed), errors.Is(err, imageresolve.ErrUnauthorized):
			status = http.StatusUnprocessableEntity
		}
		if status == http.StatusBadGateway {
			span.SetStatus(codes.Error, "image resolve failed")
		}
		writeErr(w, status, err.Error())
		return
	}
	span.SetAttributes(attribute.String("images.variant", result.Variant))
	writeJSON(w, http.StatusOK, result)
}
