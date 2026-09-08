package productanalytics

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"strings"
	"time"

	"cyclops-cs-backend/auth"
)

type IdentityClass string

const (
	IdentityInternal IdentityClass = "internal"
	IdentityExternal IdentityClass = "external"
	IdentityUnknown  IdentityClass = "unknown"
)

func ClassifyIdentity(user *auth.User) IdentityClass {
	if user == nil || strings.TrimSpace(user.ID) == "" {
		return IdentityUnknown
	}
	// Verified company-domain evidence or trusted admin-owner membership is
	// internal regardless of the authentication client (SPA, CLI, or user key).
	if user.EmailVerified && strings.HasSuffix(strings.ToLower(strings.TrimSpace(user.Email)), "@trycua.com") {
		return IdentityInternal
	}
	admin, resolved := auth.AdminSubjectMembership(user.ID)
	if !resolved {
		return IdentityUnknown
	}
	if admin {
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

// loginSessionKey returns an internal-only hash for suppressing repeated
// observations from one authenticated CLI session.
func loginSessionKey(user *auth.User) string {
	if user == nil || user.ID == "" {
		return ""
	}
	sessionID := ""
	if user.Claims != nil {
		sessionID = strings.TrimSpace(user.Claims["sid"])
		if sessionID == "" {
			sessionID = strings.TrimSpace(user.Claims["session_state"])
		}
	}
	if sessionID == "" {
		sessionID = user.ID
	}
	digest := sha256.Sum256([]byte(user.ID + "\x00" + sessionID))
	return hex.EncodeToString(digest[:])
}

const (
	EventLoginSucceeded        = "fleet_login_succeeded"
	EventPaymentGateShown      = "fleet_payment_gate_shown"
	EventPaymentSetupStart     = "fleet_payment_setup_start"
	EventPaymentMethodSetup    = "fleet_payment_method_setup"
	EventPoolCreate            = "fleet_pool_create"
	EventClaimCreate           = "fleet_claim_create"
	EventHTTPProxyRequest      = "fleet_http_proxy_request"
	EventFleetActivation       = "first_pool_with_successful_request"
	EventQualifyingWorkload    = "fleet_qualifying_workload_succeeded"
	EventResourceBlocked       = "fleet_resource_creation_blocked"
	EventQualificationRejected = "fleet_workload_qualification_rejected"
	EventAttributionBound      = "fleet_attribution_bound"

	FirstTouchCampaignIDProperty  = "fleet_first_touch_campaign_id"
	FirstTouchContentIDProperty   = "fleet_first_touch_content_id"
	FirstTouchUTMSourceProperty   = "fleet_first_touch_utm_source"
	FirstTouchUTMMediumProperty   = "fleet_first_touch_utm_medium"
	FirstTouchUTMCampaignProperty = "fleet_first_touch_utm_campaign"

	OutcomeSuccess = "success"
	OutcomeFailure = "failure"
	SourceSPA      = "spa"
	SourceCLI      = "cli"
	SourceUserKey  = "user_key"
	Version        = "1"

	ReasonNoPaymentMethod       = "no_payment_method"
	ReasonCardAdmissionRequired = "card_admission_required"
)

var allowedEvents = map[string]struct{}{
	EventLoginSucceeded: {}, EventPaymentGateShown: {}, EventPaymentSetupStart: {}, EventPaymentMethodSetup: {}, EventPoolCreate: {},
	EventClaimCreate: {}, EventHTTPProxyRequest: {},
	EventFleetActivation:    {},
	EventQualifyingWorkload: {},
	EventResourceBlocked:    {}, EventQualificationRejected: {},
	EventAttributionBound: {},
}

var allowedProperties = map[string]struct{}{
	"outcome": {}, "source": {}, "principal_type": {}, "status_code": {},
	"error_class": {}, "environment": {}, "instrumentation_version": {},
	"identity_class": {},
	"resource_type":  {}, "reason": {},
	"qualification_lookup_stage": {}, "qualification_error_class": {},
}

var allowedQualificationLookupStages = map[string]struct{}{"sandbox": {}, "claim": {}, "pool": {}, "unknown": {}}
var allowedQualificationErrorClasses = map[string]struct{}{
	"deadline_exceeded": {}, "canceled": {}, "timeout": {}, "unauthenticated": {}, "forbidden": {},
	"not_found": {}, "rate_limited": {}, "upstream_5xx": {}, "unexpected_status": {},
	"invalid_response": {}, "invalid_binding": {}, "missing_identity": {}, "invalid_request": {},
	"request_failed": {}, "unknown": {},
}

var allowedResourceTypes = map[string]struct{}{"pool": {}, "template": {}, "claim": {}}
var allowedReasons = map[string]struct{}{
	"payment_required": {}, ReasonNoPaymentMethod: {}, ReasonCardAdmissionRequired: {}, "authorization": {}, "validation": {}, "quota": {}, "timeout": {}, "internal": {},
	"not_svc_route": {}, "invalid_method": {}, "upgrade_request": {}, "binding_lookup_failed": {}, "claim_missing": {},
	"claim_mismatch": {}, "claim_not_bound": {}, "sandbox_missing": {},
	"service_mismatch": {}, "pool_lookup_failed": {}, "pool_missing": {}, "non_2xx": {}, "probe_request": {}, "facts_unavailable": {},
}

var allowedFirstTouchProperties = map[string]struct{}{
	FirstTouchCampaignIDProperty:  {},
	FirstTouchContentIDProperty:   {},
	FirstTouchUTMSourceProperty:   {},
	FirstTouchUTMMediumProperty:   {},
	FirstTouchUTMCampaignProperty: {},
}

type Event struct {
	Name       string
	DistinctID string
	InsertID   string
	Timestamp  time.Time
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
	if user.AZP == "cua-cli" && user.PrincipalType == auth.PrincipalTypeUser {
		return SourceCLI, true
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
			if _, ok := allowedFirstTouchProperties[key]; ok {
				continue
			}
			return fmt.Errorf("unsupported analytics set-once property %q", key)
		}
	}
	firstTouchCount := 0
	for key, value := range event.SetOnce {
		if _, ok := allowedFirstTouchProperties[key]; !ok {
			continue
		}
		text, ok := value.(string)
		if !ok || !validAttributionValue(text) {
			return fmt.Errorf("invalid first-touch attribution value")
		}
		firstTouchCount++
	}
	if event.Name == EventAttributionBound && firstTouchCount == 0 {
		return fmt.Errorf("attribution event requires first-touch properties")
	}
	if event.Name != EventAttributionBound && firstTouchCount > 0 {
		return fmt.Errorf("first-touch properties require attribution event")
	}
	resourceType, hasResourceType := event.Properties["resource_type"]
	if hasResourceType {
		value, ok := resourceType.(string)
		if !ok {
			return fmt.Errorf("resource type must be a string")
		}
		if _, valid := allowedResourceTypes[value]; !valid {
			return fmt.Errorf("unsupported resource type")
		}
	}
	reason, hasReason := event.Properties["reason"]
	if hasReason {
		value, ok := reason.(string)
		if !ok {
			return fmt.Errorf("analytics reason must be a string")
		}
		if _, valid := allowedReasons[value]; !valid {
			return fmt.Errorf("unsupported analytics reason")
		}
		if event.Name != EventPaymentGateShown && (value == ReasonNoPaymentMethod || value == ReasonCardAdmissionRequired) {
			return fmt.Errorf("payment gate reason requires payment gate event")
		}
	}
	if event.Name == EventResourceBlocked && (!hasResourceType || !hasReason) {
		return fmt.Errorf("resource block event requires resource type and reason")
	}
	if event.Name == EventPaymentGateShown {
		resource, resourceOK := resourceType.(string)
		reasonValue, reasonOK := reason.(string)
		if !hasResourceType || !hasReason || !resourceOK || resource != "pool" || !reasonOK || (reasonValue != ReasonNoPaymentMethod && reasonValue != ReasonCardAdmissionRequired) {
			return fmt.Errorf("payment gate event requires pool resource type and payment gate reason")
		}
	}
	if event.Name == EventQualificationRejected && !hasReason {
		return fmt.Errorf("qualification rejection event requires reason")
	}
	stage, hasStage := event.Properties["qualification_lookup_stage"]
	class, hasClass := event.Properties["qualification_error_class"]
	if hasStage || hasClass {
		stageValue, stageOK := stage.(string)
		classValue, classOK := class.(string)
		_, stageAllowed := allowedQualificationLookupStages[stageValue]
		_, classAllowed := allowedQualificationErrorClasses[classValue]
		if !stageOK || !classOK || !stageAllowed || !classAllowed {
			return fmt.Errorf("qualification lookup diagnostics require bounded stage and error class")
		}
		if event.Name != EventQualificationRejected || (reason != "binding_lookup_failed" && reason != "pool_lookup_failed") {
			return fmt.Errorf("qualification lookup diagnostics require lookup rejection")
		}
		if (reason == "pool_lookup_failed") != (stageValue == "pool") {
			return fmt.Errorf("qualification lookup stage must match rejection reason")
		}
	}
	return nil
}

func validAttributionValue(value string) bool {
	if value == "" || len(value) > 128 {
		return false
	}
	for _, b := range []byte(value) {
		if !(b >= 'a' && b <= 'z' || b >= 'A' && b <= 'Z' || b >= '0' && b <= '9' || strings.ContainsRune("._~-", rune(b))) {
			return false
		}
	}
	return true
}
