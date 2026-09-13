package auth

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// routeRequest builds a request the way RouteContext and TokenAuthMiddleware
// leave it for the authorization stage: the matched route and its params
// stamped on the context, and the User stamped on it. Params are a non-nil map
// because that is what RouteContext always stamps — it ranges the route's
// declared parameter names into a map it just made.
func routeRequest(method, path, route string, params map[string]string, body string) *http.Request {
	request := httptest.NewRequest(method, path, nil)
	if params == nil {
		params = map[string]string{}
	}
	if body != "" {
		request.Body = io.NopCloser(strings.NewReader(body))
	}
	ctx := request.Context()
	ctx = context.WithValue(ctx, routeKey, route)
	ctx = context.WithValue(ctx, paramsKey, params)
	ctx = context.WithValue(ctx, UserKey, &User{ID: "u-1", AZP: "cyclops-cs-spa"})
	return request.WithContext(ctx)
}

// TestRoutePoliciesLeaveTheRequestBodyIntact covers the axis a verdict-only
// test cannot see: whether the handler downstream of the policy stage still
// receives the bytes the client sent.
//
// PolicyMiddleware reads the body when a leaf declares maxBodyBytes, and hands
// the handler a spliced reader that replays what it consumed. The route
// policies sit on opposite sides of that:
//
//   - K8sRoutePolicy declares 1 MiB for pool admission, so it reads the body on
//     every request and this exercises the splice for real today.
//   - Every other surface declares nothing, so it never reaches loadBody and
//     passes trivially. That row is here for the day someone adds WithRawBody to
//     BasePolicy — which would put every authenticated route on the splice path
//     at once, with no verdict changing and nothing else noticing.
func TestRoutePoliciesLeaveTheRequestBodyIntact(t *testing.T) {
	LoadOpa()
	const payload = `{"spec":{"replicas":3}}`

	cases := []struct {
		name    string
		policy  Node
		request func() *http.Request
	}{
		{
			name:   "keys route reads no body",
			policy: KeysRoutePolicy(),
			request: func() *http.Request {
				return routeRequest(http.MethodPost, "/api/keys", "/api/keys", nil, payload)
			},
		},
		{
			// The path has to be one the allowlist admits, or this stops testing
			// the splice and starts testing the deny: PolicyMiddleware answers 403
			// before the handler ever reads a byte. It named a ConfigMap write
			// until #6704 made /api/k8s an allowlist, which does not admit one.
			name:   "k8s route reads the body for pool admission",
			policy: K8sRoutePolicy(),
			request: func() *http.Request {
				const path = "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims"
				return routeRequest(http.MethodPost, "/api/k8s/"+path, "/api/k8s/{path...}",
					map[string]string{"path": path}, payload)
			},
		},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			var seen string
			handler := PolicyMiddleware(testCase.policy)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				body, err := io.ReadAll(r.Body)
				if err != nil {
					t.Errorf("handler read body: %v", err)
				}
				seen = string(body)
				w.WriteHeader(http.StatusNoContent)
			}))

			response := httptest.NewRecorder()
			handler.ServeHTTP(response, testCase.request())

			if response.Code != http.StatusNoContent {
				t.Fatalf("status = %d, want %d; body = %q", response.Code, http.StatusNoContent, response.Body.String())
			}
			if seen != payload {
				t.Fatalf("handler read body %q, want %q", seen, payload)
			}
		})
	}
}
