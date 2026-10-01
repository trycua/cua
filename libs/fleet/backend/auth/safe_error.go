package auth

func NewFactUnavailableError(namespace string, cause error) error {
	if cause == nil {
		return nil
	}
	return &FactUnavailableError{Namespace: namespace, Err: cause}
}
