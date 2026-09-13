// Package middlewares wires request-scoped metadata used across auth, logging,
// and OTEL tracing.
package middlewares

import (
	"context"
	"net/http"

	"go.opentelemetry.io/contrib/instrumentation/net/http/otelhttp"
)

type ContextKey string

type originalRequestKey struct{}

func TraceMiddleware(route string, next http.Handler) http.Handler {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if original, ok := r.Context().Value(originalRequestKey{}).(*http.Request); ok {
			r = original.WithContext(r.Context())
		}
		ctx := context.WithValue(r.Context(), ContextKey("route"), route)
		next.ServeHTTP(w, r.WithContext(ctx))
	})
	traced := otelhttp.NewHandler(
		handler,
		route,
		otelhttp.WithSpanNameFormatter(func(_ string, _ *http.Request) string {
			return route
		}),
	)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		traceRequest := r.Clone(r.Context())
		traceURL := *r.URL
		traceURL.Path = route
		traceURL.RawPath = ""
		traceURL.RawQuery = ""
		traceRequest.URL = &traceURL
		traceRequest.RequestURI = route
		traceRequest = traceRequest.WithContext(context.WithValue(traceRequest.Context(), originalRequestKey{}, r))
		traced.ServeHTTP(w, traceRequest)
	})
}
