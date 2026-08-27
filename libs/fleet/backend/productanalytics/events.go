package productanalytics

import (
	"fmt"
	"strings"

	"cyclops-cs-backend/auth"
)

const (
	EventLoginSucceeded     = "fleet_login_succeeded"
	EventPaymentMethodSetup = "fleet_payment_method_setup"
	EventPoolCreate         = "fleet_pool_create"
	EventClaimCreate        = "fleet_claim_create"
	EventHTTPProxyRequest   = "fleet_http_proxy_request"

	OutcomeSuccess = "success"
	OutcomeFailure = "failure"
	SourceSPA      = "spa"
	SourceUserKey  = "user_key"
	Version        = "1"
)

var allowedEvents = map[string]struct{}{
	EventLoginSucceeded: {}, EventPaymentMethodSetup: {}, EventPoolCreate: {},
	EventClaimCreate: {}, EventHTTPProxyRequest: {},
}

var allowedProperties = map[string]struct{}{
	"outcome": {}, "source": {}, "principal_type": {}, "status_code": {},
	"error_class": {}, "environment": {}, "instrumentation_version": {},
}

type Event struct {
	Name       string
	DistinctID string
	InsertID   string
	Properties map[string]any
	SetOnce    map[string]any
}

type Capturer interface {
	Capture(Event)
}

type nopCapturer struct{}

func (nopCapturer) Capture(Event) {}

func Nop() Capturer { return nopCapturer{} }

func SourceForUser(user *auth.User, spaClientID string) (string, bool) {
	if user == nil || user.ID == "" {
		return "", false
	}
	if user.PrincipalType == auth.PrincipalTypeUserKey || strings.HasPrefix(user.AZP, "ukey-") {
		return SourceUserKey, true
	}
	if user.AZP == spaClientID || user.AZP == "oauth2-proxy" {
		return SourceSPA, true
	}
	return "", false
}

func ValidateEvent(event Event) error {
	if _, ok := allowedEvents[event.Name]; !ok {
		return fmt.Errorf("unsupported analytics event")
	}
	if event.DistinctID == "" {
		return fmt.Errorf("distinct id is required")
	}
	for key := range event.Properties {
		if _, ok := allowedProperties[key]; !ok {
			return fmt.Errorf("unsupported analytics property %q", key)
		}
	}
	for key := range event.SetOnce {
		if key != firstSeenProperty {
			return fmt.Errorf("unsupported analytics set-once property %q", key)
		}
	}
	return nil
}
