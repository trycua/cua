package productanalytics

import (
	"bufio"
	"fmt"
	"net"
	"net/http"
	"strings"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/middlewares"
)

func RouteObserver(route string, capturer Capturer, spaClientID string) auth.Middleware {
	if capturer == nil {
		capturer = Nop()
	}
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			user := auth.GetUser(r.Context())
			source, ok := SourceForUser(user, spaClientID)
			activation, tracked := activationEvent(route, r)
			if !ok || !tracked {
				next.ServeHTTP(w, r)
				return
			}
			rw := newAnalyticsResponseWriter(w)
			next.ServeHTTP(rw, r)
			status := rw.statusCode
			// Current pool creation finishes with the template request. Keep an
			// earlier warm-pool success from counting a partially-created pool.
			if activation.suppressSuccess && status >= 200 && status < 300 {
				return
			}
			outcome := OutcomeFailure
			if successfulActivation(activation.eventName, status) {
				outcome = OutcomeSuccess
			}
			principalType := user.PrincipalType
			if principalType == "" {
				principalType = auth.PrincipalTypeUser
			}
			traceID, _ := r.Context().Value(middlewares.ContextKey("traceId")).(string)
			capturer.Capture(Event{
				Name: activation.eventName, DistinctID: user.ID, InsertID: traceID,
				Properties: map[string]any{
					"outcome": outcome, "source": source, "principal_type": principalType,
					"status_code": status, "error_class": errorClass(status),
				},
			})
		})
	}
}

type activationTarget struct {
	eventName       string
	suppressSuccess bool
}

func activationEvent(route string, r *http.Request) (activationTarget, bool) {
	if strings.HasPrefix(route, "/api/svc/") {
		return activationTarget{eventName: EventHTTPProxyRequest}, true
	}
	if route != "/api/k8s/{path...}" || r.Method != http.MethodPost {
		return activationTarget{}, false
	}
	parts := strings.Split(strings.Trim(r.PathValue("path"), "/"), "/")
	if len(parts) != 6 || parts[0] != "apis" || parts[3] != "namespaces" || parts[4] == "" {
		return activationTarget{}, false
	}
	switch {
	case parts[1] == "cua.ai" && parts[2] == "v1" && parts[5] == "osgymworkspacepools":
		return activationTarget{eventName: EventPoolCreate}, true
	case parts[1] == "osgym.cua.ai" && parts[2] == "v1alpha1" && parts[5] == "osgymsandboxwarmpools":
		return activationTarget{eventName: EventPoolCreate, suppressSuccess: true}, true
	case parts[1] == "osgym.cua.ai" && parts[2] == "v1alpha1" && parts[5] == "osgymsandboxtemplates":
		return activationTarget{eventName: EventPoolCreate}, true
	case parts[1] == "osgym.cua.ai" && parts[2] == "v1alpha1" && parts[5] == "osgymsandboxclaims":
		return activationTarget{eventName: EventClaimCreate}, true
	default:
		return activationTarget{}, false
	}
}

func successfulActivation(eventName string, status int) bool {
	if eventName == EventHTTPProxyRequest {
		return status >= 200 && status < 400
	}
	return status >= 200 && status < 300
}

func errorClass(status int) string {
	switch {
	case status >= 200 && status < 400:
		return ""
	case status == http.StatusBadRequest || status == http.StatusUnprocessableEntity:
		return "validation"
	case status == http.StatusUnauthorized || status == http.StatusForbidden:
		return "authorization"
	case status == http.StatusRequestTimeout || status == http.StatusGatewayTimeout:
		return "timeout"
	case status == 499:
		return "network"
	case status >= 400 && status < 500:
		return "upstream_4xx"
	case status >= 500:
		return "upstream_5xx"
	default:
		return "internal"
	}
}

type analyticsResponseWriter struct {
	http.ResponseWriter
	statusCode int
	written    bool
}

func newAnalyticsResponseWriter(w http.ResponseWriter) *analyticsResponseWriter {
	return &analyticsResponseWriter{ResponseWriter: w, statusCode: http.StatusOK}
}

func (rw *analyticsResponseWriter) WriteHeader(status int) {
	if !rw.written {
		rw.statusCode = status
		rw.written = true
	}
	rw.ResponseWriter.WriteHeader(status)
}

func (rw *analyticsResponseWriter) Write(body []byte) (int, error) {
	if !rw.written {
		rw.WriteHeader(http.StatusOK)
	}
	return rw.ResponseWriter.Write(body)
}

func (rw *analyticsResponseWriter) Flush() {
	if flusher, ok := rw.ResponseWriter.(http.Flusher); ok {
		flusher.Flush()
	}
}

func (rw *analyticsResponseWriter) Hijack() (net.Conn, *bufio.ReadWriter, error) {
	hijacker, ok := rw.ResponseWriter.(http.Hijacker)
	if !ok {
		return nil, nil, fmt.Errorf("underlying ResponseWriter does not implement http.Hijacker")
	}
	if !rw.written {
		rw.statusCode = http.StatusSwitchingProtocols
		rw.written = true
	}
	return hijacker.Hijack()
}
