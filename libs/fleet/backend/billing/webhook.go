package billing

import (
	"encoding/json"

	"cyclops-cs-backend/productanalytics"
	"github.com/stripe/stripe-go/v85"
	"github.com/stripe/stripe-go/v85/webhook"
)

type WebhookEvent struct {
	ID              string
	Type            string
	Purpose         string
	Subject         string
	Source          string
	CustomerID      string
	PaymentMethodID string
	SetupGeneration string
}

type StripeWebhookVerifier struct{}

func NewStripeWebhookVerifier() StripeWebhookVerifier {
	return StripeWebhookVerifier{}
}

func (StripeWebhookVerifier) Verify(payload []byte, signature, secret string) (WebhookEvent, error) {
	if err := webhook.ValidatePayload(payload, signature, secret); err != nil {
		return WebhookEvent{}, err
	}
	var envelope struct {
		ID   string `json:"id"`
		Type string `json:"type"`
		Data struct {
			Object json.RawMessage `json:"object"`
		} `json:"data"`
	}
	if err := json.Unmarshal(payload, &envelope); err != nil {
		return WebhookEvent{}, nil
	}
	event := WebhookEvent{ID: envelope.ID, Type: envelope.Type}
	if envelope.Type != "setup_intent.succeeded" && envelope.Type != "setup_intent.setup_failed" {
		return event, nil
	}

	var setupIntent stripe.SetupIntent
	if err := json.Unmarshal(envelope.Data.Object, &setupIntent); err != nil {
		return event, nil
	}
	event.Purpose = setupIntent.Metadata["purpose"]
	event.SetupGeneration = setupIntent.Metadata[MetadataSetupGeneration]
	source := setupIntent.Metadata[MetadataSetupSource]
	if subject := setupIntent.Metadata[MetadataSubject]; subject != "" && (source == productanalytics.SourceSPA || source == productanalytics.SourceUserKey) {
		event.Subject = subject
		event.Source = source
	}
	if setupIntent.Customer != nil {
		event.CustomerID = setupIntent.Customer.ID
	}
	if setupIntent.PaymentMethod != nil {
		event.PaymentMethodID = setupIntent.PaymentMethod.ID
	}
	return event, nil
}
