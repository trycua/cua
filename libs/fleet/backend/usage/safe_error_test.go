package usage

import (
	"errors"
	"testing"
)

func TestNewSanitizedErrorRedactsAndPreservesCause(t *testing.T) {
	cause := errors.New("secret upstream detail")
	err := newSanitizedError("invalid allocation query webhook URL", cause)
	if err.Error() != "invalid allocation query webhook URL" {
		t.Fatalf("Error() = %q", err.Error())
	}
	if !errors.Is(err, cause) {
		t.Fatal("sanitized error lost cause identity")
	}
}
