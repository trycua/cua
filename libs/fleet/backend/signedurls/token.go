package signedurls

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/url"
	"strings"
	"time"

	"github.com/google/uuid"
)

const (
	tokenVersion   = "v1"
	maxTokenLength = 4096
)

type Capability struct {
	ID          uuid.UUID `json:"i"`
	Namespace   string    `json:"n"`
	ServiceName string    `json:"s"`
	ExpiresAt   time.Time `json:"e"`
}

type Signer struct {
	baseURL *url.URL
	secret  []byte
}

type tokenPayload struct {
	ID          uuid.UUID `json:"i"`
	Namespace   string    `json:"n"`
	ServiceName string    `json:"s"`
	ExpiresAt   int64     `json:"e"`
}

func NewSigner(baseURL string, secret []byte) (*Signer, error) {
	if len(secret) < sha256.Size {
		return nil, fmt.Errorf("signed service URL secret must be at least %d bytes", sha256.Size)
	}
	parsed, err := url.Parse(baseURL)
	if err != nil {
		return nil, fmt.Errorf("parse signed service URL base URL: %w", err)
	}
	if parsed.Scheme != "https" || parsed.Host == "" || parsed.User != nil || parsed.RawQuery != "" || parsed.Fragment != "" || (parsed.Path != "" && parsed.Path != "/") || (parsed.RawPath != "" && parsed.RawPath != "/") {
		return nil, fmt.Errorf("signed service URL base URL must be a bare HTTPS origin")
	}
	return &Signer{baseURL: parsed, secret: append([]byte(nil), secret...)}, nil
}

func (signer *Signer) URL(record Record) (string, error) {
	if signer == nil || signer.baseURL == nil || len(signer.secret) < sha256.Size {
		return "", ErrUnavailable
	}
	payload, err := json.Marshal(tokenPayload{
		ID: record.ID, Namespace: record.Namespace, ServiceName: record.ServiceName, ExpiresAt: record.ExpiresAt.Unix(),
	})
	if err != nil {
		return "", fmt.Errorf("marshal signed service capability: %w", err)
	}
	encodedPayload := base64.RawURLEncoding.EncodeToString(payload)
	signed := tokenVersion + "." + encodedPayload
	mac := hmac.New(sha256.New, signer.secret)
	_, _ = mac.Write([]byte(signed))
	token := signed + "." + base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
	publicURL := *signer.baseURL
	publicURL.Path = "/api/signed-svc/" + token + "/"
	publicURL.RawPath = ""
	return publicURL.String(), nil
}

func (signer *Signer) Verify(token string, now time.Time) (Capability, error) {
	if signer == nil || len(signer.secret) < sha256.Size || len(token) == 0 || len(token) > maxTokenLength {
		return Capability{}, ErrInvalidCapability
	}
	parts := strings.Split(token, ".")
	if len(parts) != 3 || parts[0] != tokenVersion || len(parts[1]) == 0 || len(parts[2]) == 0 {
		return Capability{}, ErrInvalidCapability
	}
	providedMAC, err := base64.RawURLEncoding.DecodeString(parts[2])
	if err != nil {
		return Capability{}, fmt.Errorf("%w: decode capability signature: %w", ErrInvalidCapability, err)
	}
	if len(providedMAC) != sha256.Size {
		return Capability{}, ErrInvalidCapability
	}
	mac := hmac.New(sha256.New, signer.secret)
	_, _ = mac.Write([]byte(parts[0] + "." + parts[1]))
	if !hmac.Equal(providedMAC, mac.Sum(nil)) {
		return Capability{}, ErrInvalidCapability
	}
	payloadBytes, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		return Capability{}, fmt.Errorf("%w: decode capability payload: %w", ErrInvalidCapability, err)
	}
	if len(payloadBytes) == 0 {
		return Capability{}, ErrInvalidCapability
	}
	var payload tokenPayload
	if err := json.Unmarshal(payloadBytes, &payload); err != nil {
		return Capability{}, fmt.Errorf("%w: decode capability payload: %w", ErrInvalidCapability, err)
	}
	if payload.ID == uuid.Nil || payload.Namespace == "" || payload.ServiceName == "" {
		return Capability{}, ErrInvalidCapability
	}
	capability := Capability{ID: payload.ID, Namespace: payload.Namespace, ServiceName: payload.ServiceName, ExpiresAt: time.Unix(payload.ExpiresAt, 0).UTC()}
	if !now.Before(capability.ExpiresAt) {
		return Capability{}, ErrInvalidCapability
	}
	return capability, nil
}
