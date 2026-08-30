package productanalytics

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"strings"

	"cyclops-cs-backend/auth"
)

type IdentityClass string

const (
	IdentityInternal IdentityClass = "internal"
	IdentityExternal IdentityClass = "external"
	IdentityUnknown  IdentityClass = "unknown"
)

func ClassifyIdentity(user *auth.User) IdentityClass {
	if user == nil || !user.EmailVerified || strings.TrimSpace(user.Email) == "" {
		return IdentityUnknown
	}
	if strings.HasSuffix(strings.ToLower(strings.TrimSpace(user.Email)), "@trycua.com") {
		return IdentityInternal
	}
	return IdentityExternal
}

func PseudonymForUserID(userID, key string) string {
	if userID == "" || key == "" {
		return ""
	}
	mac := hmac.New(sha256.New, []byte(key))
	_, _ = mac.Write([]byte(userID))
	return "u_" + hex.EncodeToString(mac.Sum(nil))
}

const (
	EventLoginSucceeded     = "fleet_login_succeeded"
	EventPaymentMethodSetup = "fleet_payment_method_setup"
	EventPoolCreate         = "fleet_pool_create"
	EventClaimCreate        = "fleet_claim_create"
	EventHTTPProxyRequest   = "fleet_http_proxy_request"
	EventFleetActivation    = "first_pool_with_successful_request"
	EventQualifyingWorkload = "fleet_qualifying_workload_succeeded"

	OutcomeSuccess = "success"
	OutcomeFailure = "failure"
	SourceSPA      = "spa"
	SourceUserKey  = "user_key"
	Version        = "1"
)

var allowedEvents = map[string]struct{}{
	EventLoginSucceeded: {}, EventPaymentMethodSetup: {}, EventPoolCreate: {},
	EventClaimCreate: {}, EventHTTPProxyRequest: {},
	EventFleetActivation:    {},
	EventQualifyingWorkload: {},
}

var allowedProperties = map[string]struct{}{
	"outcome": {}, "source": {}, "principal_type": {}, "status_code": {},
	"error_class": {}, "environment": {}, "instrumentation_version": {},
	"identity_class": {},
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
		if key != firstSeenProperty && key != firstActivationProperty {
			return fmt.Errorf("unsupported analytics set-once property %q", key)
		}
	}
	return nil
}
