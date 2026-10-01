package signedurls

import (
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
)

func TestSignerRoundTripAndDeterminism(t *testing.T) {
	signer := mustSigner(t)
	record := signedURLFixture()
	first, err := signer.URL(record)
	if err != nil {
		t.Fatalf("URL() error = %v", err)
	}
	second, err := signer.URL(record)
	if err != nil {
		t.Fatalf("URL() second error = %v", err)
	}
	if first != second {
		t.Fatalf("URL() was non-deterministic: first %q, second %q", first, second)
	}

	token := strings.TrimSuffix(strings.TrimPrefix(first, "https://run.cua.ai/api/signed-svc/"), "/")
	capability, err := signer.Verify(token, record.CreatedAt.Add(time.Minute))
	if err != nil {
		t.Fatalf("Verify() error = %v", err)
	}
	if capability.ID != record.ID || capability.Namespace != record.Namespace || capability.ServiceName != record.ServiceName || !capability.ExpiresAt.Equal(record.ExpiresAt) {
		t.Fatalf("Verify() = %#v, want record capability", capability)
	}
}

func TestSignerRejectsInvalidTokens(t *testing.T) {
	signer := mustSigner(t)
	record := signedURLFixture()
	url, err := signer.URL(record)
	if err != nil {
		t.Fatalf("URL() error = %v", err)
	}
	token := strings.TrimSuffix(strings.TrimPrefix(url, "https://run.cua.ai/api/signed-svc/"), "/")
	parts := strings.Split(token, ".")

	cases := map[string]string{
		"changed payload":   "v1." + mutate(parts[1]) + "." + parts[2],
		"changed signature": "v1." + parts[1] + "." + mutate(parts[2]),
		"unknown version":   "v2." + parts[1] + "." + parts[2],
		"oversized":         strings.Repeat("a", maxTokenLength+1),
		"bad base64url":     "v1.%%%." + parts[2],
	}
	for name, invalid := range cases {
		t.Run(name, func(t *testing.T) {
			if _, err := signer.Verify(invalid, record.CreatedAt); err == nil {
				t.Fatalf("Verify(%q) succeeded", invalid)
			}
		})
	}
}

func TestSignerRejectsExpiredCapabilities(t *testing.T) {
	signer := mustSigner(t)
	record := signedURLFixture()
	url, err := signer.URL(record)
	if err != nil {
		t.Fatalf("URL() error = %v", err)
	}
	token := strings.TrimSuffix(strings.TrimPrefix(url, "https://run.cua.ai/api/signed-svc/"), "/")
	if _, err := signer.Verify(token, record.ExpiresAt); err == nil {
		t.Fatal("Verify() accepted token at expiration")
	}
}

func TestNewSignerRejectsUnsafeConfiguration(t *testing.T) {
	secret := make([]byte, 32)
	for _, baseURL := range []string{
		"http://run.cua.ai",
		"https://user@run.cua.ai",
		"https://run.cua.ai?query=value",
		"https://run.cua.ai#fragment",
		"https://run.cua.ai/prefix",
	} {
		t.Run(baseURL, func(t *testing.T) {
			if _, err := NewSigner(baseURL, secret); err == nil {
				t.Fatalf("NewSigner(%q) succeeded", baseURL)
			}
		})
	}
	if _, err := NewSigner("https://run.cua.ai", make([]byte, 31)); err == nil {
		t.Fatal("NewSigner() accepted a short secret")
	}
}

func mustSigner(t *testing.T) *Signer {
	t.Helper()
	signer, err := NewSigner("https://run.cua.ai", []byte("01234567890123456789012345678901"))
	if err != nil {
		t.Fatalf("NewSigner() error = %v", err)
	}
	return signer
}

func signedURLFixture() Record {
	return Record{
		ID:          uuid.MustParse("8b8867c0-6b6f-4dde-921e-e541d506bc35"),
		Namespace:   "tenant-a",
		ServiceName: "sandbox-a-mcp",
		CreatedAt:   time.Date(2026, time.August, 31, 12, 0, 0, 0, time.UTC),
		ExpiresAt:   time.Date(2026, time.August, 31, 13, 0, 0, 0, time.UTC),
	}
}

func mutate(value string) string {
	if value[0] == 'a' {
		return "b" + value[1:]
	}
	return "a" + value[1:]
}
