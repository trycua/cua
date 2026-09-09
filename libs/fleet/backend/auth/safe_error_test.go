package auth

import (
	"errors"
	"testing"
)

func TestNewFactUnavailableErrorStoresOriginalCause(t *testing.T) {
	cause := errors.New("secret-provider-detail")
	err := NewFactUnavailableError("account", cause)

	var unavailable *FactUnavailableError
	if !errors.As(err, &unavailable) {
		t.Fatalf("error = %v, want *FactUnavailableError", err)
	}
	if unavailable.Err != cause {
		t.Fatalf("FactUnavailableError.Err = %T %v, want original cause", unavailable.Err, unavailable.Err)
	}
	if !errors.Is(err, cause) {
		t.Fatal("FactUnavailableError lost cause identity")
	}
}

func TestNewFactUnavailableErrorReturnsNilForNilCause(t *testing.T) {
	if err := NewFactUnavailableError("account", nil); err != nil {
		t.Fatalf("error = %v, want nil", err)
	}
}
