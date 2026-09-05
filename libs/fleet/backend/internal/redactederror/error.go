// Package redactederror preserves internal error causes without rendering them.
package redactederror

type redactedError struct {
	message string
	cause   error
}

func (err *redactedError) Error() string { return err.message }
func (err *redactedError) Unwrap() error { return err.cause }

// New renders only message; callers must use a safe, static message.
func New(message string, cause error) error {
	if cause == nil {
		return nil
	}
	return &redactedError{message: message, cause: cause}
}
