package redactederror

import (
	"errors"
	"testing"
)

type privateError struct{}

func (*privateError) Error() string { return "private upstream detail" }

func TestNewPreservesCauseWithoutRenderingIt(t *testing.T) {
	private := &privateError{}
	sentinel := errors.New("public classification")
	cause := errors.Join(sentinel, private)
	err := New("safe message", cause)
	var typed *privateError
	if err.Error() != "safe message" || !errors.Is(err, sentinel) || !errors.Is(err, private) || !errors.As(err, &typed) || typed != private || errors.Unwrap(err) != cause {
		t.Fatal("redacted wrapper must preserve the complete cause chain")
	}
	if New("safe message", nil) != nil {
		t.Fatal("nil cause must remain nil")
	}
}
