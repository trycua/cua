package config

import (
	"errors"
	"testing"
)

func TestNewSanitizedErrorRedactsAndPreservesCause(t *testing.T) {
	cause := errors.New("secret connection detail")
	err := newSanitizedError("invalid usage database URL", cause)
	if err.Error() != "invalid usage database URL" {
		t.Fatalf("Error() = %q", err.Error())
	}
	if !errors.Is(err, cause) {
		t.Fatal("sanitized error lost cause identity")
	}
}
