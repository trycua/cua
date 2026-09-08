package handlers

import (
	"context"
	"errors"
	"net"
	"net/http"
)

// attributionLookupError carries only bounded diagnostics. Never retain a raw
// transport error, URL, response body, resource name, or identity in this value.
// Readers join it with the original error to preserve the internal error chain;
// qualification extracts only these fields and never renders the joined error.
type attributionLookupError struct {
	stage string
	class string
}

func (err attributionLookupError) Error() string {
	return "qualification lookup: " + err.stage + ": " + err.class
}

func classifyAttributionLookupError(stage string, err error, fallback string) attributionLookupError {
	class := fallback
	var networkError net.Error
	switch {
	case errors.Is(err, context.DeadlineExceeded):
		class = "deadline_exceeded"
	case errors.Is(err, context.Canceled):
		class = "canceled"
	case errors.As(err, &networkError) && networkError.Timeout():
		class = "timeout"
	}
	return attributionLookupError{stage: stage, class: class}
}

func attributionLookupStatusError(stage string, status int) attributionLookupError {
	class := "unexpected_status"
	switch {
	case status == http.StatusUnauthorized:
		class = "unauthenticated"
	case status == http.StatusForbidden:
		class = "forbidden"
	case status == http.StatusNotFound:
		class = "not_found"
	case status == http.StatusTooManyRequests:
		class = "rate_limited"
	case status >= 500 && status < 600:
		class = "upstream_5xx"
	}
	return attributionLookupError{stage: stage, class: class}
}

func attributionLookupRejection(reason, fallbackStage string, err error) QualificationResult {
	diagnostic := classifyAttributionLookupError(fallbackStage, err, "unknown")
	var lookupError attributionLookupError
	if errors.As(err, &lookupError) {
		diagnostic = lookupError
	}
	return QualificationResult{Reason: reason, LookupStage: diagnostic.stage, LookupErrorClass: diagnostic.class}
}
