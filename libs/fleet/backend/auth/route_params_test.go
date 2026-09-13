package auth

import (
	"net/http"
	"net/http/httptest"
	"slices"
	"testing"
)

// TestRouteParamNamesReadsThePattern pins the scan RouteContext derives its
// parameter names with. The names are what r.PathValue is keyed by, so the two
// cases that would silently break policy input are the ones a hand-written list
// used to get right by hand: a trailing "{x...}" is keyed "x", and a pattern
// with no wildcards contributes no keys at all rather than an empty-named one.
func TestRouteParamNamesReadsThePattern(t *testing.T) {
	cases := []struct {
		route string
		want  []string
	}{
		{"/api/keys", nil},
		{"/healthz", nil},
		{"/api/keys/{id}", []string{"id"}},
		{"/api/k8s/{path...}", []string{"path"}},
		{"/api/svc/{namespace}/{service}", []string{"namespace", "service"}},
		{"/api/svc/{namespace}/{service}/{path...}", []string{"namespace", "service", "path"}},
		// "{$}" anchors a pattern to the end of the path and binds no value,
		// so it must not become a parameter the policy sees as undefined.
		{"/api/keys/{$}", nil},
	}

	for _, testCase := range cases {
		got := routeParamNames(testCase.route)
		if !slices.Equal(got, testCase.want) {
			t.Errorf("routeParamNames(%q) = %v, want %v", testCase.route, got, testCase.want)
		}
	}
}

// TestRouteContextStampsTheWildcardsTheMuxMatched runs the derivation through a
// real ServeMux: the names the scan produces have to be the names the mux
// actually bound, or every PathValue lookup returns "" and the policy reads a
// map of empty strings.
func TestRouteContextStampsTheWildcardsTheMuxMatched(t *testing.T) {
	const route = "/api/svc/{namespace}/{service}/{path...}"

	var got map[string]string
	mux := http.NewServeMux()
	mux.Handle(route, RouteContext(route)(http.HandlerFunc(func(_ http.ResponseWriter, r *http.Request) {
		got, _ = r.Context().Value(paramsKey).(map[string]string)
	})))

	mux.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest(http.MethodGet, "/api/svc/ns-a/svc-a/v1/status", nil))

	want := map[string]string{"namespace": "ns-a", "service": "svc-a", "path": "v1/status"}
	if len(got) != len(want) {
		t.Fatalf("params = %v, want %v", got, want)
	}
	for key, value := range want {
		if got[key] != value {
			t.Errorf("params[%q] = %q, want %q (full: %v)", key, got[key], value, got)
		}
	}
}
