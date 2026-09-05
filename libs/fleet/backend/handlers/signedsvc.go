package handlers

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"time"

	"cyclops-cs-backend/signedurls"
)

type signedSvcResolver interface {
	Resolve(context.Context, string, time.Time) (signedurls.Record, error)
}

func (h Handlers) HandleSignedSvc(w http.ResponseWriter, r *http.Request) {
	token := r.PathValue("token")
	resolver, ok := h.signedServiceURLService().(signedSvcResolver)
	if !ok || token == "" {
		http.NotFound(w, r)
		return
	}

	record, err := resolver.Resolve(r.Context(), token, time.Now().UTC())
	if errors.Is(err, signedurls.ErrInvalidCapability) {
		http.NotFound(w, r)
		return
	}
	if errors.Is(err, signedurls.ErrUnavailable) {
		http.Error(w, "signed service unavailable", http.StatusServiceUnavailable)
		return
	}
	if err != nil {
		http.Error(w, "signed service resolution failed", http.StatusInternalServerError)
		return
	}
	if !validSignedServiceURLIdentifier(record.Namespace) || !validSignedServiceURLIdentifier(record.ServiceName) {
		http.NotFound(w, r)
		return
	}
	exists, err := h.signedServiceExistsFor(r.Context(), record.Namespace, record.ServiceName, "")
	if err != nil {
		http.Error(w, "signed service lookup unavailable", http.StatusBadGateway)
		return
	}
	if !exists {
		http.NotFound(w, r)
		return
	}
	slog.Info("signed service capability resolved", "url_id", record.ID)

	upstreamPath := normalizeProxyUpstreamPath(r.PathValue("path"))

	h.proxyService(
		w,
		r,
		record.Namespace,
		record.ServiceName,
		upstreamPath,
		"/api/signed-svc/"+token,
		"signed-svc",
	)
}
