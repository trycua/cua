package auth

import "context"

// OwnedNamespace returns the namespace a request's ownership check is about:
// the {namespace} parameter where the route binds one, and {name} otherwise.
// It returns "" when the route names no namespace at all.
//
// This is the Go half of target_namespace in authz_ownership.rego. Two halves
// exist because the answer is needed on both sides — the Rego compares it
// against the token's claims, and the fact provider probes it — and neither can
// read the other's. TestOwnedNamespaceMatchesRego evaluates the rule and this
// function over the same parameter maps, so the two cannot drift apart quietly.
func OwnedNamespace(ctx context.Context) string {
	params, _ := ctx.Value(paramsKey).(map[string]string)
	if namespace, ok := params["namespace"]; ok {
		return namespace
	}
	return params["name"]
}
