package auth

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"log/slog"
	"net/http"
	"slices"
	"strings"
	"sync"

	"cyclops-cs-backend/featureflagadmin"
	"cyclops-cs-backend/middlewares"
)

type Middleware func(http.Handler) http.Handler

// PolicySource describes where Rego text comes from. modules() performs the
// I/O; sourceKey() is a stable structural identity for telling whether two
// leaves are provably the same, which a future dedupe pass will need.
// sourceKey returns "" to mean "no stable identity", and any caller comparing
// keys must treat "" as never equal to anything.
type PolicySource interface {
	policySource()
	modules() ([]policyModule, error)
	sourceKey() string
}

type policyModule struct {
	name   string
	source string
}

type inlinePolicySource struct {
	name   string
	source string
}

func (inlinePolicySource) policySource() {}

func (source inlinePolicySource) modules() ([]policyModule, error) {
	if source.name == "" || source.source == "" {
		return nil, fmt.Errorf("inline policy requires a name and source")
	}
	return []policyModule{{name: source.name, source: source.source}}, nil
}

// sourceKey digests the policy text with SHA-256 rather than a fast
// non-cryptographic hash: a collision here would let a future dedupe pass
// merge two leaves that run different Rego, which in authorization code is a
// correctness bug. The cost is paid once, at startup.
func (source inlinePolicySource) sourceKey() string {
	digest := sha256.Sum256([]byte(source.source))
	return fmt.Sprintf("inline:%s:%x", source.name, digest)
}

func Inline(name, source string) PolicySource {
	return inlinePolicySource{name: name, source: source}
}

type fsPolicySource struct {
	filesystem fs.FS
	path       string
}

func (fsPolicySource) policySource() {}

func (source fsPolicySource) modules() ([]policyModule, error) {
	contents, err := fs.ReadFile(source.filesystem, source.path)
	if err != nil {
		return nil, fmt.Errorf("read policy %q: %w", source.path, err)
	}
	return []policyModule{{name: source.path, source: string(contents)}}, nil
}

// sourceKey is empty because two different fs.FS values can share a path, so
// a path alone cannot prove two leaves are the same. A false merge would be a
// correctness bug.
func (fsPolicySource) sourceKey() string { return "" }

func FromFS(filesystem fs.FS, path string) PolicySource {
	return fsPolicySource{filesystem: filesystem, path: path}
}

var policyModuleRegistry = struct {
	sync.RWMutex
	modules map[string]policyModule
}{modules: map[string]policyModule{}}

func RegisterPolicyModule(name, filename, source string) {
	if name == "" || filename == "" || source == "" {
		panic("registered OPA policy requires a name, filename, and source")
	}
	policyModuleRegistry.Lock()
	defer policyModuleRegistry.Unlock()
	policyModuleRegistry.modules[name] = policyModule{name: filename, source: source}
}

type registeredPolicySource struct {
	name string
}

func (registeredPolicySource) policySource() {}

func (source registeredPolicySource) modules() ([]policyModule, error) {
	policyModuleRegistry.RLock()
	module, ok := policyModuleRegistry.modules[source.name]
	policyModuleRegistry.RUnlock()
	if !ok {
		return nil, fmt.Errorf("OPA policy module %q is not registered", source.name)
	}
	return []policyModule{module}, nil
}

func (source registeredPolicySource) sourceKey() string {
	return "registered:" + source.name
}

func Registered(name string) PolicySource {
	return registeredPolicySource{name: name}
}

type multiPolicySource struct {
	sources []PolicySource
}

func (multiPolicySource) policySource() {}

func (source multiPolicySource) modules() ([]policyModule, error) {
	var modules []policyModule
	for _, child := range source.sources {
		childModules, err := child.modules()
		if err != nil {
			return nil, err
		}
		modules = append(modules, childModules...)
	}
	return modules, nil
}

// sourceKey is order-sensitive, because module order is part of what gets
// compiled. Children are quoted before joining so the encoding stays
// injective: without quoting, children {"X,Y", "Z"} and {"X", "Y,Z"} would
// produce the same key.
func (source multiPolicySource) sourceKey() string {
	keys := make([]string, 0, len(source.sources))
	for _, child := range source.sources {
		key := child.sourceKey()
		if key == "" {
			return ""
		}
		keys = append(keys, fmt.Sprintf("%q", key))
	}
	return "multi[" + strings.Join(keys, ",") + "]"
}

func Modules(sources ...PolicySource) PolicySource {
	if len(sources) == 0 {
		panic("OPA Modules requires at least one source")
	}
	return multiPolicySource{sources: sources}
}

type PolicyOption func(*policyConfig)

type FactSet map[string]any

type FactProvider interface {
	CacheKey() string
	LoadFacts(context.Context, *http.Request) (FactSet, error)
}

type factProviderConfig struct {
	namespace string
	provider  FactProvider
}

type policyConfig struct {
	query         string
	factProviders []factProviderConfig
	maxBodyBytes  int64
}

func Query(query string) PolicyOption {
	return func(config *policyConfig) {
		config.query = query
	}
}

func WithFacts(namespace string, provider FactProvider) PolicyOption {
	return func(config *policyConfig) {
		if namespace == "" || provider == nil || provider.CacheKey() == "" {
			panic("OPA fact provider requires a namespace and cache key")
		}
		config.factProviders = append(config.factProviders, factProviderConfig{
			namespace: namespace,
			provider:  provider,
		})
	}
}

func WithRawBody(maxBytes int64) PolicyOption {
	return func(config *policyConfig) {
		if maxBytes <= 0 {
			panic("OPA raw body limit must be positive")
		}
		config.maxBodyBytes = maxBytes
	}
}

type cachedFacts struct {
	facts FactSet
	err   error
}

type requestPolicyInput struct {
	request *http.Request
	base    map[string]any
	// bodyBudget is the whole plan's largest raw-body limit, computed at
	// compile time. Every leaf reads against it rather than against its own
	// limit, so no leaf's limit leaks into what its siblings can see.
	bodyBudget int64
	factCache  map[string]cachedFacts
	bodyRead   bool
	body       []byte
	bodyErr    error
}

func newRequestPolicyInput(request *http.Request, bodyBudget int64) *requestPolicyInput {
	*request = *request.WithContext(context.WithValue(request.Context(), requestClockSnapshotKey{}, &requestClockSnapshot{}))
	user, _ := request.Context().Value(UserKey).(*User)
	route, _ := request.Context().Value(routeKey).(string)
	params, _ := request.Context().Value(paramsKey).(map[string]string)
	return &requestPolicyInput{
		request:    request,
		bodyBudget: bodyBudget,
		factCache:  map[string]cachedFacts{},
		base: map[string]any{
			"method": request.Method,
			"path":   request.URL.Path,
			"route":  route,
			"params": params,
			"user":   buildUserInput(user),
			"flags":  authorizationFlags(request.Context()),
		},
	}
}

func (input *requestPolicyInput) forPolicy(ctx context.Context, config policyConfig) (map[string]any, error) {
	document := make(map[string]any, len(input.base)+2)
	for key, value := range input.base {
		document[key] = value
	}

	facts := map[string]any{}
	for _, configured := range config.factProviders {
		value, err := input.resolveFacts(ctx, configured.provider)
		if err != nil {
			return nil, err
		}
		merged, ok := facts[configured.namespace].(map[string]any)
		if !ok {
			merged = map[string]any{}
			facts[configured.namespace] = merged
		}
		for key, fact := range value {
			if _, exists := merged[key]; exists {
				return nil, fmt.Errorf("fact namespace %q contains duplicate key %q", configured.namespace, key)
			}
			merged[key] = fact
		}
	}
	document["facts"] = facts

	if config.maxBodyBytes > 0 {
		body, err := input.loadBody(config.maxBodyBytes)
		if err != nil {
			return nil, err
		}
		document["body"] = string(body)
	}
	return document, nil
}

func (input *requestPolicyInput) resolveFacts(ctx context.Context, provider FactProvider) (FactSet, error) {
	cacheKey := provider.CacheKey()
	if cached, ok := input.factCache[cacheKey]; ok {
		return cached.facts, cached.err
	}
	facts, err := provider.LoadFacts(ctx, input.request)
	input.factCache[cacheKey] = cachedFacts{facts: facts, err: err}
	return facts, err
}

type policyBodyError struct {
	message string
}

func (err policyBodyError) Error() string {
	return err.message
}

// FactUnavailableError is how a FactProvider says "the system I ask was
// unreachable", as opposed to "it answered no" — which is a FactSet, not an
// error — or "this process is misassembled", which is a plain error.
//
// The distinction is the response. Both are undecidable and both fail closed,
// but an unreachable dependency is a 502 the caller should retry, and anything
// else is a 500 somebody should look at. Before the namespace-ownership check
// moved into the policy, the Go handler answered its probe failures with
// exactly that 502; routing it through the lattice would otherwise have turned
// every apiserver hiccup into a 500 and pointed the alert at the wrong system.
//
// It is exported because providers live outside this package — the one that
// exists is a handlers method — and errors.As is how PolicyMiddleware
// recognises it, through whatever the fold's errors.Join wrapped it in.
type FactUnavailableError struct {
	// Namespace is the input.facts key the provider was loading, so the log
	// names which fact could not be answered rather than only which route.
	Namespace string
	Err       error
}

func (err *FactUnavailableError) Error() string {
	return fmt.Sprintf("fact %q unavailable", err.Namespace)
}

func (err *FactUnavailableError) Unwrap() error { return err.Err }

// splicedBody reads a prefix already consumed from a request body and then the
// rest of that body, while Close still targets the original.
type splicedBody struct {
	io.Reader
	io.Closer
}

// spliceBody rebuilds the body the downstream handler will read: the bytes
// already consumed, followed by whatever is left of the original.
//
// It is unconditional on purpose. Whenever the prefix is the whole body the
// original is drained and the splice reads exactly like a reader over the
// prefix, so there is nothing to gain from a special case — while every
// condition that could guard it is a way for the handler to receive a stream
// with its first bytes missing. That includes the read-error path: replaying
// the prefix and letting the original re-surface its error is more faithful
// than discarding bytes that were read successfully.
//
// Close passes through rather than being swallowed by a NopCloser because this
// wrapper does not own the original's lifetime and the original may still hold
// unread bytes. Under net/http nothing leaks either way — the server closes the
// reqBody it captured, whatever a handler or middleware substitutes — but a
// wrapper that silently drops Close is wrong for any caller that is not the
// server.
func spliceBody(prefix []byte, original io.ReadCloser) io.ReadCloser {
	return splicedBody{
		Reader: io.MultiReader(bytes.NewReader(prefix), original),
		Closer: original,
	}
}

// loadBody reads the request body at most once, always up to the plan-wide
// budget, and never mutates shared state based on the calling leaf's limit.
// Reading stays lazy: a short-circuited branch never triggers it.
func (input *requestPolicyInput) loadBody(maxBytes int64) ([]byte, error) {
	if !input.bodyRead {
		input.bodyRead = true
		if input.request.Body != nil {
			original := input.request.Body
			input.body, input.bodyErr = io.ReadAll(io.LimitReader(original, input.bodyBudget+1))
			input.request.Body = spliceBody(input.body, original)
		}
	}
	bodyErr := input.bodyErr
	if bodyErr != nil {
		return input.body, bodyErr
	}
	if int64(len(input.body)) > maxBytes {
		return input.body, policyBodyError{message: "request body exceeds policy limit"}
	}
	return input.body, nil
}

type MiddlewareOption func(*middlewareConfig)

type middlewareConfig struct {
	deniedMessage  string
	deniedAudit    *deniedAuditConfig
	adminAPIErrors bool
	freshAdminAuth bool
	// pipeline names the optimizer passes to run before compiling. pipelineSet
	// distinguishes "run the default pipeline" from "run nothing": the two are
	// both an empty slice, and only the flag tells them apart.
	pipeline    []string
	pipelineSet bool
}

func WithFreshAdminAuthorization() MiddlewareOption {
	return func(config *middlewareConfig) {
		config.freshAdminAuth = true
	}
}

func WithAdminAPIErrorResponses() MiddlewareOption {
	return func(config *middlewareConfig) {
		config.adminAPIErrors = true
	}
}

func WithDeniedMessage(message string) MiddlewareOption {
	return func(config *middlewareConfig) {
		config.deniedMessage = message
	}
}

type deniedAuditConfig struct {
	event    string
	maxBytes int64
}

// WithDeniedAudit records a bounded, typed summary of rejected mutation
// attempts. It is intentionally generic so every privileged surface can use
// the same safe behavior without retaining raw request bodies.
func WithDeniedAudit(event string, maxBytes int64) MiddlewareOption {
	return func(config *middlewareConfig) {
		config.deniedAudit = &deniedAuditConfig{event: event, maxBytes: maxBytes}
	}
}

// WithPipeline overrides the optimizer passes for this middleware.
// WithPipeline() with no arguments disables optimization entirely.
//
// Names are checked against the optimizer registry when the pipeline runs, so
// a typo panics during route construction rather than silently skipping a pass.
func WithPipeline(names ...string) MiddlewareOption {
	return func(config *middlewareConfig) {
		// Copied, not aliased. Spreading an existing slice into a variadic
		// parameter shares its backing array, so storing names directly would
		// leave the config holding a live view of the caller's slice.
		config.pipeline = slices.Clone(names)
		config.pipelineSet = true
	}
}

// PolicyMiddleware optimizes the policy AST, compiles it into an executable
// plan, and enforces that plan on every request. Compilation failures panic so
// a bad policy aborts the process during route construction, before it serves
// traffic; so does a pipeline naming a pass that does not exist.
func PolicyMiddleware(expression Node, options ...MiddlewareOption) Middleware {
	config := middlewareConfig{deniedMessage: "forbidden"}
	for _, option := range options {
		option(&config)
	}
	pipeline := DefaultPipeline()
	if config.pipelineSet {
		pipeline = config.pipeline
	}
	compiled, err := Compile(Optimize(expression, pipeline))
	if err != nil {
		panic(fmt.Sprintf("compile policy: %v", err))
	}
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, request *http.Request) {
			if config.freshAdminAuth {
				request = withFreshAdminFlags(request)
			}
			verdict := compiled.eval(request.Context(), newRequestPolicyInput(request, compiled.bodyBudget))
			switch verdict.truth {
			case truthTrue:
				next.ServeHTTP(w, request)
			case truthFalse:
				if config.deniedAudit != nil && request.Method != http.MethodGet {
					logRejectedMutation(request, *config.deniedAudit, "not_admin")
				}
				message := verdict.reason
				if message == "" {
					message = config.deniedMessage
				}
				writePolicyError(w, config, http.StatusForbidden, "not_admin", message)
			default:
				// truthError, and deliberately also any truth value a future
				// change adds: an undecidable policy must fail closed rather
				// than fall through to the handler.
				//
				// Every arm here answers with a fixed string, so these are the
				// only places the actual failure is named. Without them an
				// undecidable policy is a 5xx with no cause recorded anywhere,
				// at any level.
				var bodyErr policyBodyError
				var factErr *FactUnavailableError
				switch {
				case errors.As(verdict.err, &bodyErr):
					if config.deniedAudit != nil && request.Method != http.MethodGet {
						logRejectedMutation(request, *config.deniedAudit, "invalid_request")
					}
					writePolicyError(w, config, http.StatusBadRequest, "invalid_request", "could not inspect request")
				case errors.As(verdict.err, &factErr):
					if config.deniedAudit != nil && request.Method != http.MethodGet {
						logRejectedMutation(request, *config.deniedAudit, "authorization_unavailable")
					}
					// A dependency the policy consults is down. Warn rather than
					// Error: nothing here is broken, and the alert this should
					// raise is the dependency's, not ours.
					slog.Warn("opa: authorization fact unavailable",
						"class", "dependency_unavailable",
						"retryable", true,
						"facts", factErr.Namespace,
						"route", request.Context().Value(routeKey),
						"traceId", request.Context().Value(middlewares.ContextKey("traceId")))
					writePolicyError(w, config, http.StatusBadGateway, "authorization_unavailable", "authorization check unavailable")
				default:
					if config.deniedAudit != nil && request.Method != http.MethodGet {
						logRejectedMutation(request, *config.deniedAudit, "policy_error")
					}
					slog.Error("opa: policy evaluation failed",
						"class", "policy_evaluation_failed",
						"retryable", false,
						"route", request.Context().Value(routeKey),
						"traceId", request.Context().Value(middlewares.ContextKey("traceId")))
					writePolicyError(w, config, http.StatusInternalServerError, "policy_error", "policy evaluation failed")
				}
			}
		})
	}
}

func writePolicyError(w http.ResponseWriter, config middlewareConfig, status int, code, message string) {
	if !config.adminAPIErrors {
		writeJSONErr(w, status, message)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]any{"code": code, "message": message})
}

func logDeniedMutation(request *http.Request, config deniedAuditConfig) {
	logRejectedMutation(request, config, "not_admin")
}

func logRejectedMutation(request *http.Request, config deniedAuditConfig, reason string) {
	details := map[string]any{}
	parseFailure := ""
	if request.Body != nil && config.maxBytes > 0 {
		body, err := io.ReadAll(io.LimitReader(request.Body, config.maxBytes))
		var extra [1]byte
		extraCount, extraErr := request.Body.Read(extra[:])
		request.Body = splicedBody{Reader: io.MultiReader(bytes.NewReader(body), bytes.NewReader(extra[:extraCount]), request.Body), Closer: request.Body}
		switch {
		case err != nil:
			parseFailure = "read_failed"
		case extraErr != io.EOF || extraCount != 0:
			parseFailure = "body_too_large"
		case len(body) > 0:
			if err := json.Unmarshal(body, &details); err != nil {
				parseFailure = "invalid_json"
			}
		}
	}
	user, _ := request.Context().Value(UserKey).(*User)
	key := request.PathValue("key")
	if key == "" && request.Method == http.MethodPost && parseFailure == "" {
		if bodyKey, ok := details["key"].(string); ok && bodyKey != "" && featureflagadmin.BoundedAuditKey(bodyKey) == bodyKey {
			key = bodyKey
		}
	}
	attrs := []any{
		"event", config.event,
		"actor", "",
		"actor_email", "",
		"principal_type", "",
		"operation", strings.ToLower(request.Method),
		"path", featureflagadmin.BoundedAuditPath(request.URL.Path, request.PathValue("key")),
		"traceId", request.Context().Value(middlewares.ContextKey("traceId")),
		"result", "rejected",
		"reason", reason,
	}
	if key != "" {
		attrs = append(attrs, "key", featureflagadmin.BoundedAuditKey(key))
	}
	if user != nil {
		attrs[3] = user.ID
		attrs[5] = user.Email
		attrs[7] = user.PrincipalType
	}
	if parseFailure != "" {
		attrs = append(attrs, "parse_failure", parseFailure)
	} else {
		if value, ok := details["value"]; ok {
			attrs = append(attrs, "attempted_value", value)
		}
		if valueType, ok := details["value_type"]; ok {
			attrs = append(attrs, "value_type", valueType)
		}
		if expectedVersion, ok := details["expected_version"]; ok {
			attrs = append(attrs, "expected_version", expectedVersion)
		}
	}
	slog.Warn("authorization mutation rejected", attrs...)
}
