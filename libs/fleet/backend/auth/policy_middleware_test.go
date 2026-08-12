package auth

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"testing"
	"testing/fstest"
	"testing/iotest"
)

func TestPolicyMiddlewareInlinePolicyAllowsRequest(t *testing.T) {
	expression := Policy(
		Inline("inline-allow.rego", `
package inline_allow

default allow = false

allow {
	input.method == "GET"
}
`),
		Query("data.inline_allow.allow"),
	)

	called := false
	handler := PolicyMiddleware(expression)(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		called = true
		w.WriteHeader(http.StatusNoContent)
	}))

	request := httptest.NewRequest(http.MethodGet, "/resource", nil)
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)

	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusNoContent, response.Body.String())
	}
	if !called {
		t.Fatal("allowed request did not reach downstream handler")
	}
}

func TestPolicyExpressionComposition(t *testing.T) {
	allow := Policy(Inline("allow.rego", `package allow_policy
allow { true }
`), Query("data.allow_policy.allow"))
	deny := Policy(Inline("deny.rego", `package deny_policy
default allow = false
`), Query("data.deny_policy.allow"))

	cases := []struct {
		name       string
		expression PolicyExpression
		wantStatus int
	}{
		{
			name:       "nested any groups satisfy all",
			expression: All(Any(deny, allow), Any(allow, deny)),
			wantStatus: http.StatusNoContent,
		},
		{
			name:       "all denies when one child denies",
			expression: All(allow, deny),
			wantStatus: http.StatusForbidden,
		},
		{
			name:       "any denies when every child denies",
			expression: Any(deny, deny),
			wantStatus: http.StatusForbidden,
		},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			handler := PolicyMiddleware(testCase.expression)(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
				w.WriteHeader(http.StatusNoContent)
			}))
			response := httptest.NewRecorder()
			handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/", nil))
			if response.Code != testCase.wantStatus {
				t.Fatalf("status = %d, want %d; body = %s", response.Code, testCase.wantStatus, response.Body.String())
			}
		})
	}
}

func TestPolicySourceFromFS(t *testing.T) {
	filesystem := fstest.MapFS{
		"policies/fs.rego": {Data: []byte(`package fs_policy
allow { input.path == "/from-fs" }
`)},
	}
	expression := Policy(
		FromFS(filesystem, "policies/fs.rego"),
		Query("data.fs_policy.allow"),
	)
	handler := PolicyMiddleware(expression)(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/from-fs", nil))
	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusNoContent, response.Body.String())
	}
}

func TestPolicyModulesCanImportRegisteredModule(t *testing.T) {
	RegisterPolicyModule("test-base", "base.rego", `package test_base
is_get { input.method == "GET" }
`)
	expression := Policy(
		Modules(
			Registered("test-base"),
			Inline("consumer.rego", `package consumer
import data.test_base
allow { test_base.is_get }
`),
		),
		Query("data.consumer.allow"),
	)
	handler := PolicyMiddleware(expression)(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/", nil))
	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusNoContent, response.Body.String())
	}
}

type testFactProvider struct {
	cacheKey string
	facts    FactSet
	err      error
	loads    int
}

func (provider *testFactProvider) CacheKey() string {
	return provider.cacheKey
}

func (provider *testFactProvider) LoadFacts(context.Context, *http.Request) (FactSet, error) {
	provider.loads++
	return provider.facts, provider.err
}

func TestPolicyMiddlewareLoadsAndCachesFacts(t *testing.T) {
	provider := &testFactProvider{
		cacheKey: "user-facts",
		facts: FactSet{
			"age":    24,
			"groups": []string{"customers", "verified-users"},
		},
	}
	agePolicy := Policy(
		Inline("age.rego", `package age_policy
allow { input.facts.user.age >= 21 }
`),
		Query("data.age_policy.allow"),
		WithFacts("user", provider),
	)
	groupPolicy := Policy(
		Inline("group.rego", `package group_policy
allow { input.facts.user.groups[_] == "verified-users" }
`),
		Query("data.group_policy.allow"),
		WithFacts("user", provider),
	)

	handler := PolicyMiddleware(All(agePolicy, groupPolicy))(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/", nil))

	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusNoContent, response.Body.String())
	}
	if provider.loads != 1 {
		t.Fatalf("provider loads = %d, want 1", provider.loads)
	}
}

func TestPolicyMiddlewareSkipsFactsInShortCircuitedBranch(t *testing.T) {
	provider := &testFactProvider{cacheKey: "unused", facts: FactSet{"age": 24}}
	allow := Policy(Inline("fast.rego", `package fast
allow { true }
`), Query("data.fast.allow"))
	unused := Policy(
		Inline("unused.rego", `package unused
allow { input.facts.user.age >= 21 }
`),
		Query("data.unused.allow"),
		WithFacts("user", provider),
	)

	handler := PolicyMiddleware(Any(allow, unused))(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/", nil))

	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusNoContent)
	}
	if provider.loads != 0 {
		t.Fatalf("provider loads = %d, want 0", provider.loads)
	}
}

func TestPolicyMiddlewareFactProviderErrorFailsClosed(t *testing.T) {
	provider := &testFactProvider{cacheKey: "broken", err: errors.New("database unavailable")}
	expression := Policy(
		Inline("facts.rego", `package facts
allow { input.facts.user.age >= 21 }
`),
		Query("data.facts.allow"),
		WithFacts("user", provider),
	)

	called := false
	handler := PolicyMiddleware(expression)(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		called = true
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/", nil))

	if response.Code != http.StatusInternalServerError {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusInternalServerError)
	}
	if called {
		t.Fatal("provider error reached downstream handler")
	}
}

func TestPolicyMiddlewareProvidesAndRestoresRawBody(t *testing.T) {
	const body = `{"spec":{"runtime":"macos"}}`
	expression := Policy(
		Inline("body.rego", `package body_policy
allow { contains(input.body, "macos") }
`),
		Query("data.body_policy.allow"),
		WithRawBody(1024),
	)

	var downstreamBody string
	handler := PolicyMiddleware(expression, WithDeniedMessage("body denied"))(http.HandlerFunc(func(w http.ResponseWriter, request *http.Request) {
		contents, err := io.ReadAll(request.Body)
		if err != nil {
			t.Fatalf("read downstream body: %v", err)
		}
		downstreamBody = string(contents)
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	request := httptest.NewRequest(http.MethodPost, "/", strings.NewReader(body))
	handler.ServeHTTP(response, request)

	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusNoContent, response.Body.String())
	}
	if downstreamBody != body {
		t.Fatalf("downstream body = %q, want %q", downstreamBody, body)
	}
}

func TestPolicyMiddlewareAnySatisfiedDisjunctSurvivesSiblingError(t *testing.T) {
	provider := &testFactProvider{cacheKey: "broken-any-allow", err: errors.New("database unavailable")}
	broken := Policy(
		Inline("broken-any.rego", `package broken_any
allow { input.facts.user.age >= 21 }
`),
		Query("data.broken_any.allow"),
		WithFacts("user", provider),
	)
	allow := Policy(Inline("fallback.rego", `package fallback
allow { true }
`), Query("data.fallback.allow"))

	handler := PolicyMiddleware(Any(broken, allow))(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/", nil))

	// A disjunction with a satisfied disjunct is true regardless of whether a
	// sibling could be evaluated. True annihilates max.
	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusNoContent, response.Body.String())
	}
}

func TestPolicyMiddlewareAnyFailsClosedWhenNothingAllows(t *testing.T) {
	provider := &testFactProvider{cacheKey: "broken-any-deny", err: errors.New("database unavailable")}
	broken := Policy(
		Inline("broken-any-deny.rego", `package broken_any_deny
allow { input.facts.user.age >= 21 }
`),
		Query("data.broken_any_deny.allow"),
		WithFacts("user", provider),
	)
	deny := Policy(Inline("deny-any.rego", `package deny_any
default allow = false
`), Query("data.deny_any.allow"))

	handler := PolicyMiddleware(Any(broken, deny))(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/", nil))

	// No disjunct allowed, so the error survives and the request fails closed.
	if response.Code != http.StatusInternalServerError {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusInternalServerError, response.Body.String())
	}
}

func TestPolicyMiddlewareAllDenyBeatsSiblingError(t *testing.T) {
	provider := &testFactProvider{cacheKey: "broken-all", err: errors.New("database unavailable")}
	broken := Policy(
		Inline("broken-all.rego", `package broken_all
allow { input.facts.user.age >= 21 }
`),
		Query("data.broken_all.allow"),
		WithFacts("user", provider),
	)
	deny := Policy(Inline("deny-all.rego", `package deny_all
default allow = false
`), Query("data.deny_all.allow"))

	handler := PolicyMiddleware(All(broken, deny))(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/", nil))

	// False annihilates min: a definite deny outranks an unevaluable sibling.
	if response.Code != http.StatusForbidden {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusForbidden, response.Body.String())
	}
}

// permutations returns every ordering of values. It is used to assert that
// evaluation is order-independent, which is the property that licenses the
// optimizer to reorder children — over policy nodes here, and over optimizer
// pass names in policy_optimize_test.go, where the same question is asked of
// the pipeline.
func permutations[T any](values []T) [][]T {
	if len(values) <= 1 {
		return [][]T{slices.Clone(values)}
	}
	var result [][]T
	for index := range values {
		rest := make([]T, 0, len(values)-1)
		rest = append(rest, values[:index]...)
		rest = append(rest, values[index+1:]...)
		for _, tail := range permutations(rest) {
			result = append(result, append([]T{values[index]}, tail...))
		}
	}
	return result
}

func TestPolicyMiddlewareEvaluationIsOrderIndependent(t *testing.T) {
	// Leaves with differing WithRawBody limits belong here. The plan-wide body
	// budget makes each leaf's limit check a pure function of the body length
	// and that leaf's own limit, so a small limit somewhere in the tree can no
	// longer decide what a wide-limit sibling sees. Every request below carries
	// the same body so the body-reading leaves actually exercise that.
	const requestBody = "0123456789"

	allow := Policy(Inline("comm-allow.rego", `package comm_allow
allow { true }
`), Query("data.comm_allow.allow"))
	deny := Policy(Inline("comm-deny.rego", `package comm_deny
default allow = false
`), Query("data.comm_deny.allow"))
	broken := Policy(
		Inline("comm-broken.rego", `package comm_broken
allow { input.facts.user.age >= 21 }
`),
		Query("data.comm_broken.allow"),
		WithFacts("user", &testFactProvider{cacheKey: "comm-broken", err: errors.New("database unavailable")}),
	)
	alsoBroken := Policy(
		Inline("comm-broken-2.rego", `package comm_broken_2
allow { input.facts.other.age >= 21 }
`),
		Query("data.comm_broken_2.allow"),
		WithFacts("other", &testFactProvider{cacheKey: "comm-broken-2", err: errors.New("api unavailable")}),
	)
	// Three body-reading leaves at differing limits. bodyOverLimit's limit is
	// smaller than the request body, so it is the leaf that used to poison the
	// shared read for its siblings whenever it ran first.
	bodyAllow := Policy(
		// Built from requestBody rather than repeating the literal, so the
		// const really is the single source it claims to be: change the request
		// body and this leaf follows it instead of silently going false.
		Inline("comm-body-allow.rego", fmt.Sprintf(`package comm_body_allow
allow { input.body == %q }
`, requestBody)),
		Query("data.comm_body_allow.allow"),
		WithRawBody(1024),
	)
	bodyDeny := Policy(
		Inline("comm-body-deny.rego", `package comm_body_deny
allow { input.body == "not the request body" }
`),
		Query("data.comm_body_deny.allow"),
		WithRawBody(512),
	)
	bodyOverLimit := Policy(
		Inline("comm-body-over.rego", `package comm_body_over
allow { true }
`),
		Query("data.comm_body_over.allow"),
		WithRawBody(2),
	)

	cases := []struct {
		name     string
		children []Node
		// min: any False -> deny, else any Error -> error status, else allow.
		// max: any True -> allow, else any Error -> error status, else deny.
		// A surviving error is 400 when a policyBodyError is anywhere in the
		// join and 500 otherwise.
		wantAll int
		wantAny int
	}{
		{"allow and deny", []Node{allow, deny}, http.StatusForbidden, http.StatusNoContent},
		{"broken and allow", []Node{broken, allow}, http.StatusInternalServerError, http.StatusNoContent},
		{"broken and deny", []Node{broken, deny}, http.StatusForbidden, http.StatusInternalServerError},
		{"broken allow and deny", []Node{broken, allow, deny}, http.StatusForbidden, http.StatusNoContent},
		{"two broken and allow", []Node{broken, alsoBroken, allow}, http.StatusInternalServerError, http.StatusNoContent},
		{"two broken and deny", []Node{broken, alsoBroken, deny}, http.StatusForbidden, http.StatusInternalServerError},
		{"body allow and body over limit", []Node{bodyAllow, bodyOverLimit}, http.StatusBadRequest, http.StatusNoContent},
		{"body deny and body over limit", []Node{bodyDeny, bodyOverLimit}, http.StatusForbidden, http.StatusBadRequest},
		{"body allow body deny and body over limit", []Node{bodyAllow, bodyDeny, bodyOverLimit}, http.StatusForbidden, http.StatusNoContent},
		{"body over limit and bodyless allow", []Node{bodyOverLimit, allow}, http.StatusBadRequest, http.StatusNoContent},
		{"body over limit and broken facts", []Node{bodyOverLimit, broken}, http.StatusBadRequest, http.StatusBadRequest},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			for _, operator := range []struct {
				name   string
				build  func(...Node) Node
				status int
			}{
				{"All", All, testCase.wantAll},
				{"Any", Any, testCase.wantAny},
			} {
				t.Run(operator.name, func(t *testing.T) {
					// Errorf, not Fatalf: which subset of orderings disagrees
					// is the whole diagnosis. Stopping at the first one throws
					// that away.
					for _, ordering := range permutations(testCase.children) {
						handler := PolicyMiddleware(operator.build(ordering...))(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
							w.WriteHeader(http.StatusNoContent)
						}))
						response := httptest.NewRecorder()
						handler.ServeHTTP(response, httptest.NewRequest(http.MethodPost, "/", strings.NewReader(requestBody)))
						if response.Code != operator.status {
							t.Errorf("status = %d, want %d; body = %s; plan:\n%s",
								response.Code, operator.status, strings.TrimSpace(response.Body.String()),
								Explain(operator.build(ordering...)))
						}
					}
				})
			}
		})
	}
}

func TestPolicyMiddlewarePanicsOnEmptyConjunction(t *testing.T) {
	// The guard has to surface at route construction, not at request time:
	// a route that cannot be authorized must never serve traffic at all.
	defer func() {
		recovered := recover()
		if recovered == nil {
			t.Fatal("PolicyMiddleware built a route from an empty conjunction, which would allow every request")
		}
		// Assert why it panicked. A test satisfied by any panic keeps passing
		// once it is no longer the guard doing the panicking.
		if message := fmt.Sprint(recovered); !strings.Contains(message, "auth.AllNode has no children") {
			t.Fatalf("panic = %v, want it to name the childless node", message)
		}
	}()

	PolicyMiddleware(AllNode{})
}

// undecidablePolicy compiles and then fails at evaluation: two complete rules
// assigning different values to the same name is an eval_conflict_error, and
// making both bodies depend on input keeps OPA from resolving the conflict
// before a request arrives.
const undecidablePolicy = `package undecidable

allow = true {
	input.method != ""
}

allow = false {
	input.method != ""
}
`

// TestPolicyMiddlewareLogsTheEvaluationError pins the one thing the 500 does
// not carry. The response body is the fixed string "policy evaluation failed",
// so unless the middleware logs verdict.err an undecidable policy is an error
// with its cause recorded nowhere, at no level — which is what this code did
// until the test existed.
//
// slog.Default is swapped rather than injected because PolicyMiddleware has no
// logger seam, and this package forbids t.Parallel (see policy_optimize.go),
// which is what makes swapping a global safe here.
func TestPolicyMiddlewareLogsTheEvaluationError(t *testing.T) {
	var logged bytes.Buffer
	restore := slog.Default()
	slog.SetDefault(slog.New(slog.NewJSONHandler(&logged, nil)))
	t.Cleanup(func() { slog.SetDefault(restore) })

	called := false
	handler := PolicyMiddleware(Policy(
		Inline("undecidable.rego", undecidablePolicy),
		Query("data.undecidable.allow"),
	))(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		called = true
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/", nil))

	if response.Code != http.StatusInternalServerError {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusInternalServerError)
	}
	if called {
		t.Fatal("an evaluation error reached the downstream handler")
	}
	// The specific OPA error code, not merely that something was logged: a
	// record naming only the route would satisfy a laxer check while still
	// losing the cause.
	if !strings.Contains(logged.String(), "eval_conflict_error") {
		t.Fatalf("the evaluation error was not logged; got:\n%s", logged.String())
	}
}

func TestPolicyMiddlewareBodyErrorClassificationIsOrderIndependent(t *testing.T) {
	// Exactly one leaf reads the body, so this exercises error accumulation:
	// a joined error is classified by the body error inside it whichever
	// sibling produced it. The separate hazard of leaves at differing limits
	// sharing one read belongs to TestPolicyBodyIsRestoredForDownstreamHandler
	// and to the body rows of TestPolicyMiddlewareEvaluationIsOrderIndependent.
	tooLarge := Policy(
		Inline("order-body.rego", `package order_body
allow { input.body == "12345" }
`),
		Query("data.order_body.allow"),
		WithRawBody(4),
	)
	broken := Policy(
		Inline("order-facts.rego", `package order_facts
allow { input.facts.user.age >= 21 }
`),
		Query("data.order_facts.allow"),
		WithFacts("user", &testFactProvider{cacheKey: "order-facts", err: errors.New("database unavailable")}),
	)

	// Both siblings error, one with a policyBodyError. Errors are joined, so
	// errors.As finds the body error whichever sibling produced it first.
	for _, ordering := range [][]Node{{tooLarge, broken}, {broken, tooLarge}} {
		for _, build := range []func(...Node) Node{All, Any} {
			handler := PolicyMiddleware(build(ordering...))(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
				w.WriteHeader(http.StatusNoContent)
			}))
			response := httptest.NewRecorder()
			handler.ServeHTTP(response, httptest.NewRequest(http.MethodPost, "/", strings.NewReader("12345")))
			if response.Code != http.StatusBadRequest {
				t.Fatalf("status = %d, want %d; body = %s; plan:\n%s",
					response.Code, http.StatusBadRequest, strings.TrimSpace(response.Body.String()),
					Explain(build(ordering...)))
			}
		}
	}
}

func TestPolicyBodyIsRestoredForDownstreamHandler(t *testing.T) {
	const requestBody = "0123456789"

	// narrow's own limit is under the body length, so it errors; wide's is well
	// over it and its Rego checks the bytes, so a satisfied disjunct proves the
	// wide leaf saw the body intact. Running both orderings is what shows the
	// narrow leaf can no longer decide what its sibling reads.
	narrow := Policy(
		Inline("narrow-restore.rego", `package narrow_restore
allow { true }
`),
		Query("data.narrow_restore.allow"),
		WithRawBody(2),
	)
	wide := Policy(
		Inline("wide-restore.rego", fmt.Sprintf(`package wide_restore
allow { input.body == %q }
`, requestBody)),
		Query("data.wide_restore.allow"),
		WithRawBody(1024),
	)

	for _, testCase := range []struct {
		name string
		node Node
	}{
		{"narrow first", Any(narrow, wide)},
		{"wide first", Any(wide, narrow)},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			tracked := &closeTrackingBody{Reader: strings.NewReader(requestBody)}
			var seen string
			handler := PolicyMiddleware(testCase.node)(http.HandlerFunc(func(w http.ResponseWriter, request *http.Request) {
				body, err := io.ReadAll(request.Body)
				if err != nil {
					t.Fatalf("read downstream body: %v", err)
				}
				seen = string(body)
				if err := request.Body.Close(); err != nil {
					t.Fatalf("close downstream body: %v", err)
				}
				w.WriteHeader(http.StatusNoContent)
			}))
			response := httptest.NewRecorder()
			handler.ServeHTTP(response, httptest.NewRequest(http.MethodPost, "/", tracked))

			if response.Code != http.StatusNoContent {
				t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusNoContent, response.Body.String())
			}
			if seen != requestBody {
				t.Fatalf("downstream body = %q, want the full request body", seen)
			}
			// The replacement body wraps the original rather than a NopCloser
			// over a detached buffer, so the handler's Close reaches it. This
			// is the assertion that keeps that deliberate.
			if tracked.closes != 1 {
				t.Fatalf("original body closes = %d, want 1", tracked.closes)
			}
		})
	}
}

func TestPolicyBodyReadErrorStillGivesHandlerTheBytesItRead(t *testing.T) {
	// A body that yields some bytes and then fails. The reading leaf latches
	// the transport error and is undecidable, but a body-less sibling allows
	// the request anyway — and the handler must then get the bytes that were
	// read plus the same error, not a stream with its first bytes eaten. This
	// is the read-error corner of the restore, the one place the old code
	// still skipped it.
	failure := errors.New("connection reset mid-body")
	reading := Policy(Inline("read-error.rego", `package read_error
allow { true }
`), Query("data.read_error.allow"), WithRawBody(1024))
	bodyless := Policy(Inline("read-error-allow.rego", `package read_error_allow
allow { true }
`), Query("data.read_error_allow.allow"))

	tracked := &closeTrackingBody{
		Reader: io.MultiReader(strings.NewReader("0123"), iotest.ErrReader(failure)),
	}
	var seen string
	var readErr error
	handler := PolicyMiddleware(Any(reading, bodyless))(http.HandlerFunc(func(w http.ResponseWriter, request *http.Request) {
		body, err := io.ReadAll(request.Body)
		seen, readErr = string(body), err
		if err := request.Body.Close(); err != nil {
			t.Fatalf("close downstream body: %v", err)
		}
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodPost, "/", tracked))

	// The satisfied disjunct annihilates the reading leaf's error.
	if response.Code != http.StatusNoContent {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusNoContent, response.Body.String())
	}
	if seen != "0123" {
		t.Fatalf("downstream body = %q, want the bytes read before the failure", seen)
	}
	if !errors.Is(readErr, failure) {
		t.Fatalf("downstream read error = %v, want it to surface %v", readErr, failure)
	}
	if tracked.closes != 1 {
		t.Fatalf("original body closes = %d, want 1", tracked.closes)
	}
}

// closeTrackingBody is a request body that counts Close calls, so a test can
// tell whether the ReadCloser the middleware substitutes still releases the
// original.
type closeTrackingBody struct {
	io.Reader
	closes int
}

func (body *closeTrackingBody) Close() error {
	body.closes++
	return nil
}

func TestPolicyBodyOverBudgetReachesHandlerIntact(t *testing.T) {
	// The body is larger than every limit in the plan, so the leaf that reads
	// it errors. A body-less sibling can still allow the request, and once it
	// does, the handler is entitled to the body exactly as sent. The bytes
	// loadBody consumed to make its own limit decision must be spliced back in
	// front of the remainder, not silently dropped: a proxied request arriving
	// with a hole at the front is worse than no request at all.
	overLimit := Policy(
		Inline("over-budget.rego", `package over_budget
allow { true }
`),
		Query("data.over_budget.allow"),
		WithRawBody(2),
	)
	bodyless := Policy(Inline("over-budget-allow.rego", `package over_budget_allow
allow { true }
`), Query("data.over_budget_allow.allow"))

	// The allowing sibling annihilates the disjunction, so ordering decides
	// whether the body is read at all: with it first, nothing touches the body
	// and the handler gets the original untouched. Both orderings must hand the
	// handler the same bytes and close the original exactly once.
	for _, testCase := range []struct {
		name string
		node Node
	}{
		{"reading leaf first", Any(overLimit, bodyless)},
		{"allowing leaf first", Any(bodyless, overLimit)},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			tracked := &closeTrackingBody{Reader: strings.NewReader("0123456789")}
			var seen string
			handler := PolicyMiddleware(testCase.node)(http.HandlerFunc(func(w http.ResponseWriter, request *http.Request) {
				body, err := io.ReadAll(request.Body)
				if err != nil {
					t.Fatalf("read downstream body: %v", err)
				}
				seen = string(body)
				if err := request.Body.Close(); err != nil {
					t.Fatalf("close downstream body: %v", err)
				}
				w.WriteHeader(http.StatusNoContent)
			}))
			response := httptest.NewRecorder()
			handler.ServeHTTP(response, httptest.NewRequest(http.MethodPost, "/", tracked))

			if response.Code != http.StatusNoContent {
				t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusNoContent, response.Body.String())
			}
			if seen != "0123456789" {
				t.Fatalf("downstream body = %q, want the full request body", seen)
			}
			// The original still holds unread bytes here, so the substituted
			// ReadCloser must not swallow Close: it does not own that stream.
			if tracked.closes != 1 {
				t.Fatalf("original body closes = %d, want 1", tracked.closes)
			}
		})
	}
}

func TestPolicyMiddlewareCachedBodyHonorsEachLeafLimit(t *testing.T) {
	wide := Policy(
		Inline("wide.rego", `package wide
allow { input.body == "12345" }
`),
		Query("data.wide.allow"),
		WithRawBody(1024),
	)
	narrow := Policy(
		Inline("narrow.rego", `package narrow
allow { input.body == "12345" }
`),
		Query("data.narrow.allow"),
		WithRawBody(4),
	)

	handler := PolicyMiddleware(All(wide, narrow))(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodPost, "/", strings.NewReader("12345")))

	if response.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, http.StatusBadRequest, response.Body.String())
	}
}
