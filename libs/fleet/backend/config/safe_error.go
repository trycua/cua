package config

type sanitizedError struct {
	message string
	cause   error
}

func (err *sanitizedError) Error() string { return err.message }

func (err *sanitizedError) Unwrap() error { return err.cause }

func newSanitizedError(message string, cause error) error {
	if cause == nil {
		return nil
	}
	return &sanitizedError{message: message, cause: cause}
}
