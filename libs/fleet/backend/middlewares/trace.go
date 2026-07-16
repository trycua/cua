// Package middlewares wires request-scoped metadata used across auth, logging,
// and OTEL tracing.
package middlewares

import (
	"context"
	"net/http"

	"go.opentelemetry.io/contrib/instrumentation/net/http/otelhttp"
)

type ContextKey string

func TraceMiddleware(route string, next http.Handler) http.Handler {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		ctx := context.WithValue(r.Context(), ContextKey("route"), route)
		next.ServeHTTP(w, r.WithContext(ctx))
	})

	return otelhttp.NewHandler(
		handler,
		route,
		otelhttp.WithSpanNameFormatter(func(_ string, _ *http.Request) string {
			return route
		}),
	)
}
