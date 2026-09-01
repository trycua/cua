package productanalytics

import (
	"bufio"
	"bytes"
	"context"
	"fmt"
	"net"
	"net/http"
	"strings"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/middlewares"
	"go.opentelemetry.io/otel/trace"
)

type SvcQualifier func(context.Context, *http.Request, int) bool
type SvcQualificationResult struct {
	Qualifies bool
	Reason    string
}
type SvcQualification func(context.Context, *http.Request, int) SvcQualificationResult

func RouteObserver(route string, capturer Capturer, spaClientID string, qualifiers ...SvcQualifier) auth.Middleware {
	var qualifier SvcQualification
	if len(qualifiers) > 0 && qualifiers[0] != nil {
		qualifier = func(ctx context.Context, r *http.Request, status int) SvcQualificationResult {
			return SvcQualificationResult{Qualifies: qualifiers[0](ctx, r, status)}
		}
	}
	return routeObserver(route, capturer, spaClientID, qualifier)
}

func RouteObserverWithQualification(route string, capturer Capturer, spaClientID string, qualifier SvcQualification) auth.Middleware {
	return routeObserver(route, capturer, spaClientID, qualifier)
}

func routeObserver(route string, capturer Capturer, spaClientID string, qualifier SvcQualification) auth.Middleware {
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
			traceID := trace.SpanContextFromContext(r.Context()).TraceID().String()
			if traceID == "00000000000000000000000000000000" {
				traceID, _ = r.Context().Value(middlewares.ContextKey("traceId")).(string)
			}
			if activation.eventName == EventHTTPProxyRequest && ClassifyIdentity(user) == IdentityExternal && qualifier != nil {
				go func() {
					defer func() { _ = recover() }()
					ctx, cancel := context.WithTimeout(context.WithoutCancel(r.Context()), 250*time.Millisecond)
					defer cancel()
					result := qualifier(ctx, r, status)
					if !result.Qualifies {
						if result.Reason != "" {
							capturer.Capture(Event{Name: EventQualificationRejected, DistinctID: user.ID, InsertID: traceID + ":qualification-rejected", Properties: map[string]any{
								"outcome": OutcomeFailure, "source": source, "principal_type": user.PrincipalType, "status_code": status,
								"error_class": errorClass(status), "identity_class": IdentityExternal, "reason": result.Reason,
							}})
						}
						return
					}
					properties := map[string]any{
						"outcome": OutcomeSuccess, "source": source, "principal_type": user.PrincipalType,
						"status_code": status, "error_class": "", "identity_class": IdentityExternal,
					}
					capturer.Capture(Event{
						Name: EventQualifyingWorkload, DistinctID: user.ID, InsertID: traceID,
						Properties: properties,
					})
					capturer.Capture(Event{
						Name: EventFleetActivation, DistinctID: user.ID, InsertID: "fleet-activation:" + user.ID,
						SetOnce:    map[string]any{firstActivationProperty: time.Now().UTC().Format(time.RFC3339)},
						Properties: properties,
					})
				}()
			}
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
			capturer.Capture(Event{
				Name: activation.eventName, DistinctID: user.ID, InsertID: traceID,
				Properties: map[string]any{
					"outcome": outcome, "source": source, "principal_type": principalType,
					"status_code": status, "error_class": errorClass(status),
					"identity_class": ClassifyIdentity(user),
				},
			})
			if activation.resourceType != "" && (status < 200 || status >= 300) {
				reason := blockedReason(status, rw.paymentRequired)
				capturer.Capture(Event{Name: EventResourceBlocked, DistinctID: user.ID, InsertID: traceID + ":resource-blocked", Properties: map[string]any{
					"outcome": OutcomeFailure, "source": source, "principal_type": principalType, "status_code": status,
					"error_class": errorClass(status), "identity_class": ClassifyIdentity(user), "resource_type": activation.resourceType, "reason": reason,
				}})
			}
		})
	}
}

type activationTarget struct {
	eventName       string
	suppressSuccess bool
	resourceType    string
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
		return activationTarget{eventName: EventPoolCreate, resourceType: "pool"}, true
	case parts[1] == "osgym.cua.ai" && parts[2] == "v1alpha1" && parts[5] == "osgymsandboxwarmpools":
		return activationTarget{eventName: EventPoolCreate, suppressSuccess: true, resourceType: "pool"}, true
	case parts[1] == "osgym.cua.ai" && parts[2] == "v1alpha1" && parts[5] == "osgymsandboxtemplates":
		return activationTarget{eventName: EventPoolCreate, resourceType: "template"}, true
	case parts[1] == "osgym.cua.ai" && parts[2] == "v1alpha1" && parts[5] == "osgymsandboxclaims":
		return activationTarget{eventName: EventClaimCreate, resourceType: "claim"}, true
	default:
		return activationTarget{}, false
	}
}

func blockedReason(status int, paymentRequired bool) string {
	if paymentRequired {
		return "payment_required"
	}
	switch {
	case status == http.StatusBadRequest || status == http.StatusUnprocessableEntity:
		return "validation"
	case status == http.StatusUnauthorized || status == http.StatusForbidden:
		return "authorization"
	case status == http.StatusTooManyRequests:
		return "quota"
	case status == http.StatusRequestTimeout || status == http.StatusGatewayTimeout:
		return "timeout"
	case status >= 500:
		return "internal"
	default:
		return "authorization"
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
	statusCode      int
	written         bool
	paymentRequired bool
	bodySample      []byte
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
	if len(rw.bodySample) < 512 {
		remaining := 512 - len(rw.bodySample)
		if len(body) < remaining {
			remaining = len(body)
		}
		rw.bodySample = append(rw.bodySample, body[:remaining]...)
		if bytes.Contains(rw.bodySample, []byte("A payment method is required to create this resource")) {
			rw.paymentRequired = true
		}
	}
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
