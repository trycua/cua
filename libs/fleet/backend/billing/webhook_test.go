package billing

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"testing"
	"time"
)

func billingStripeSignature(payload []byte, secret string, timestamp int64) string {
	mac := hmac.New(sha256.New, []byte(secret))
	_, _ = fmt.Fprintf(mac, "%d.", timestamp)
	_, _ = mac.Write(payload)
	return fmt.Sprintf("t=%d,v1=%s", timestamp, hex.EncodeToString(mac.Sum(nil)))
}

func TestVerifyParsesFleetSetupIntentSucceeded(t *testing.T) {
	secret := "whsec_test"
	payload := []byte(`{
	  "id":"evt_setup",
	  "type":"setup_intent.succeeded",
	  "data":{"object":{
	    "id":"seti_123",
	    "object":"setup_intent",
	    "customer":"cus_owned",
	    "payment_method":"pm_card",
	    "metadata":{"purpose":"fleet_default_card","fleet_subject":"subject-1","fleet_source":"spa"}
	  }}
	}`)
	signature := billingStripeSignature(payload, secret, time.Now().Unix())

	event, err := NewStripeWebhookVerifier().Verify(payload, signature, secret)
	if err != nil {
		t.Fatalf("Verify() error = %v", err)
	}
	want := WebhookEvent{
		ID: "evt_setup", Type: "setup_intent.succeeded",
		Purpose: SetupPurpose, Subject: "subject-1", Source: "spa", CustomerID: "cus_owned", PaymentMethodID: "pm_card",
	}
	if event != want {
		t.Fatalf("event = %#v, want %#v", event, want)
	}
}

func TestVerifyLeavesMalformedSetupIntentUnprojected(t *testing.T) {
	secret := "whsec_test"
	payload := []byte(`{"id":"evt_setup","type":"setup_intent.succeeded"}`)
	signature := billingStripeSignature(payload, secret, time.Now().Unix())

	event, err := NewStripeWebhookVerifier().Verify(payload, signature, secret)
	if err != nil {
		t.Fatalf("Verify() error = %v", err)
	}
	want := WebhookEvent{ID: "evt_setup", Type: "setup_intent.succeeded"}
	if event != want {
		t.Fatalf("event = %#v, want %#v", event, want)
	}
}

func TestVerifyIgnoresCorrectlySignedTruncatedEnvelope(t *testing.T) {
	secret := "whsec_test"
	payload := []byte(`{"id":"evt_truncated"`)
	signature := billingStripeSignature(payload, secret, time.Now().Unix())

	event, err := NewStripeWebhookVerifier().Verify(payload, signature, secret)
	if err != nil {
		t.Fatalf("Verify() error = %v", err)
	}
	if event != (WebhookEvent{}) {
		t.Fatalf("event = %#v, want empty event", event)
	}
}

func TestVerifyParsesServerSetupGeneration(t *testing.T) {
	secret := "whsec_test"
	payload := []byte(`{"id":"evt_setup","type":"setup_intent.succeeded","data":{"object":{"id":"seti_123","object":"setup_intent","customer":"cus_owned","payment_method":"pm_card","metadata":{"purpose":"fleet_default_card","fleet_setup_generation":"server-generated-token"}}}}`)
	signature := billingStripeSignature(payload, secret, time.Now().Unix())

	event, err := NewStripeWebhookVerifier().Verify(payload, signature, secret)
	if err != nil {
		t.Fatalf("Verify() error = %v", err)
	}
	if event.SetupGeneration != "server-generated-token" {
		t.Fatalf("setup generation = %q, want server-generated-token", event.SetupGeneration)
	}
}

func TestVerifyParsesFleetSetupIntentFailedAttribution(t *testing.T) {
	secret := "whsec_test"
	payload := []byte(`{"id":"evt_failed","type":"setup_intent.setup_failed","data":{"object":{"id":"seti_123","object":"setup_intent","metadata":{"purpose":"fleet_default_card","fleet_subject":"subject-1","fleet_source":"spa","fleet_setup_generation":"generation-1"}}}}`)
	signature := billingStripeSignature(payload, secret, time.Now().Unix())
	event, err := NewStripeWebhookVerifier().Verify(payload, signature, secret)
	if err != nil {
		t.Fatalf("Verify() error = %v", err)
	}
	want := WebhookEvent{ID: "evt_failed", Type: "setup_intent.setup_failed", Purpose: SetupPurpose, Subject: "subject-1", Source: "spa", SetupGeneration: "generation-1"}
	if event != want {
		t.Fatalf("event = %#v, want %#v", event, want)
	}
}

func TestVerifyDropsUnsupportedSetupSource(t *testing.T) {
	secret := "whsec_test"
	payload := []byte(`{"id":"evt_failed","type":"setup_intent.setup_failed","data":{"object":{"id":"seti_123","object":"setup_intent","metadata":{"purpose":"fleet_default_card","fleet_subject":"subject-1","fleet_source":"github"}}}}`)
	signature := billingStripeSignature(payload, secret, time.Now().Unix())
	event, err := NewStripeWebhookVerifier().Verify(payload, signature, secret)
	if err != nil {
		t.Fatalf("Verify() error = %v", err)
	}
	if event.Subject != "" || event.Source != "" {
		t.Fatalf("untrusted attribution = subject %q source %q", event.Subject, event.Source)
	}
}
