package auth

// SanitizedError keeps an internal cause available to errors.Is/errors.As while
// exposing only a reviewed message at rendering boundaries.
type SanitizedError struct {
	Message string
	Cause   error
}

func (err SanitizedError) Error() string { return err.Message }
func (err SanitizedError) Unwrap() error { return err.Cause }

func NewSanitizedError(message string, cause error) error {
	if cause == nil {
		return nil
	}
	return SanitizedError{Message: message, Cause: cause}
}

func NewFactUnavailableError(namespace, message string, cause error) error {
	return &FactUnavailableError{
		Namespace: namespace,
		Err:       NewSanitizedError(message, cause),
	}
}
