package auth

import (
	"context"
	_ "embed"
	"encoding/json"
	"log"
	"log/slog"
	"net/http"
	"strings"
	"sync"
	"time"

	"cyclops-cs-backend/middlewares"

	"github.com/open-feature/go-sdk/openfeature"
	"github.com/open-policy-agent/opa/rego"
	"github.com/trycua/cloud/pkg/featureflags"
	"golang.org/x/sync/singleflight"
)

type ctxKey int

const UserKey ctxKey = 1

var opaQuery *rego.PreparedEvalQuery
var opaEventQuery *rego.PreparedEvalQuery
var opaAdminQuery *rego.PreparedEvalQuery
var opaPoolAdmissionQuery *rego.PreparedEvalQuery

//go:embed authz.rego
var authzPolicy string

//go:embed filters.rego
var filtersPolicy string

//go:embed pool_admission.rego
var poolAdmissionPolicy string

// flagPrefix is the OpenFeature path under which the auth package's OPA
// flags live. The dev SimpleEnvProvider maps it to env vars prefixed
// CYCLOPS_CS_ (stripping "/feature-flags/", lowercasing nothing,
// replacing "/" and "-" with "_"); the aws-ssm provider uses the path
// as the SSM Parameter Store prefix directly.
//
//	/feature-flags/cyclops-cs/admin-subs             → CYCLOPS_CS_ADMIN_SUBS
//	/feature-flags/cyclops-cs/event-reason-allowlist → CYCLOPS_CS_EVENT_REASON_ALLOWLIST
//
// Values are JSON string arrays (e.g. '["sub1","sub2"]').
const flagPrefix = "/feature-flags/cyclops-cs/"

// flagsTTL bounds how long flagsData returns its last computed result
// before re-resolving via OpenFeature. Refresh is lazy and ad-hoc: the
// next caller that arrives after expiry triggers a re-resolve on the
// request path — there is no background ticker. Flags are passed into
// each OPA evaluation as `input.flags` (see OpaMiddleware / EvalIsAdmin /
// EvalVisibleEvents), so a refreshed value takes effect on the very next
// request without rebuilding any prepared query or restarting the pod.
const flagsTTL = 1 * time.Minute

var (
	flagsMu    sync.Mutex
	flagsValue map[string]interface{}
	flagsExp   time.Time

	// flagsSF collapses concurrent refreshes (e.g. a burst of requests
	// arriving right after expiry, or at cold start) into a single
	// compute, so we never fan out N simultaneous SSM round-trips.
	flagsSF singleflight.Group

	// ffClient is the lazily-initialised OpenFeature client used by
	// computeFlagsData. NewClient is cheap, but sync.Once gives us a
	// single shared handle across concurrent first-callers and pins the
	// client name to one place.
	ffClientOnce sync.Once
	ffClient     *openfeature.Client
)

// flagsData returns the resolved feature-flag map (admin_subs,
// event_reason_allowlist, …), TTL-cached at flagsTTL. On a hit it returns
// the cached map under the lock; on a miss it resolves via OpenFeature
// behind a singleflight so only one goroutine fans out to SSM while the
// rest share the result. The map is meant to be placed under input.flags
// for OPA; the policy references a subset of keys (admin_subs /
// event_reason_allowlist), extras are ignored, and absent keys evaluate as
// undefined (implicitly denying / redacting).
//
// The OpenFeature provider is installed by featureflags.SetupProvider at
// startup (main.go) — aws-ssm in prod, SimpleEnvProvider in dev, with the
// contrib from-env provider as a degraded fallback.
func flagsData() map[string]interface{} {
	flagsMu.Lock()
	if flagsValue != nil && time.Now().Before(flagsExp) {
		cached := flagsValue
		flagsMu.Unlock()
		return cached
	}
	flagsMu.Unlock()

	v, _, _ := flagsSF.Do("flags", func() (interface{}, error) {
		// Re-check under the lock: a concurrent refresh may have just
		// populated the cache while we waited to enter singleflight.
		flagsMu.Lock()
		if flagsValue != nil && time.Now().Before(flagsExp) {
			cached := flagsValue
			flagsMu.Unlock()
			return cached, nil
		}
		flagsMu.Unlock()

		// Resolve with a detached context so a single caller's cancelled
		// request can't abort the refresh shared by everyone else.
		result := computeFlagsData(context.Background())

		flagsMu.Lock()
		flagsValue = result
		flagsExp = time.Now().Add(flagsTTL)
		flagsMu.Unlock()
		return result, nil
	})
	return v.(map[string]interface{})
}

func computeFlagsData(ctx context.Context) map[string]interface{} {
	ffClientOnce.Do(func() {
		ffClient = openfeature.NewClient("cyclops-cs-auth")
	})
	keys, err := featureflags.ListFlagKeys(ctx, flagPrefix)
	if err != nil {
		slog.Warn("auth: flag discovery failed; flags will be empty", "prefix", flagPrefix, "err", err)
	}
	flags := map[string]interface{}{}
	for _, key := range keys {
		name := opaNameFromFlagKey(key)
		if name == "" {
			continue
		}
		flags[name] = loadStringList(ctx, ffClient, key)
	}
	return flags
}

// opaNameFromFlagKey maps a discovered flag path to the Rego identifier
// used by the policy (under input.flags). Nested paths under flagPrefix are
// skipped — the policy only references single-leaf flag names.
//
//	/feature-flags/cyclops-cs/admin-subs → admin_subs
func opaNameFromFlagKey(key string) string {
	leaf := strings.TrimPrefix(key, flagPrefix)
	if leaf == "" || strings.Contains(leaf, "/") {
		return ""
	}
	return strings.ReplaceAll(leaf, "-", "_")
}

// loadStringList resolves a JSON-array flag to []interface{} for OPA's
// input.flags document. The value is StringValue with default "[]"; a
// malformed payload yields an empty list and a warn log so OPA still
// evaluates.
//
// A 3s per-call deadline keeps an unreachable Parameter Store from
// stalling pod startup; the from-env fallback kicks in on timeout.
func loadStringList(ctx context.Context, client *openfeature.Client, flagKey string) []interface{} {
	callCtx, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()
	raw, err := client.StringValue(callCtx, flagKey, "[]", openfeature.EvaluationContext{})
	if err != nil {
		slog.Warn("auth: flag eval failed; using empty list", "flag", flagKey, "err", err)
		return []interface{}{}
	}
	var items []string
	if err := json.Unmarshal([]byte(raw), &items); err != nil {
		slog.Warn("auth: flag value is not a JSON string array; using empty list", "flag", flagKey, "err", err)
		return []interface{}{}
	}
	out := make([]interface{}, len(items))
	for i, v := range items {
		out[i] = v
	}
	return out
}

// LoadOpa prepares the embedded Rego policy and all derived queries.
// Feature flags are NOT baked into an OPA data store here; they are passed
// per-request as input.flags (see flagsData), so prepared queries stay
// valid across flag refreshes. The OpenFeature provider must be installed
// by featureflags.SetupProvider before this runs.
func LoadOpa() {
	// authz queries only need authz.rego (is_admin lives there too).
	prepareAuthzQuery := func(q string) *rego.PreparedEvalQuery {
		pq, err := rego.New(
			rego.Query(q),
			rego.Module("authz.rego", authzPolicy),
		).PrepareForEval(context.Background())
		if err != nil {
			log.Fatalf("opa: prepare authz %q: %v", q, err)
		}
		return &pq
	}

	// filter queries need both authz.rego (for data.authz.is_admin) and filters.rego
	prepareFilterQuery := func(q string) *rego.PreparedEvalQuery {
		pq, err := rego.New(
			rego.Query(q),
			rego.Module("authz.rego", authzPolicy),
			rego.Module("filters.rego", filtersPolicy),
		).PrepareForEval(context.Background())
		if err != nil {
			log.Fatalf("opa: prepare filter %q: %v", q, err)
		}
		return &pq
	}

	opaQuery = prepareAuthzQuery("data.authz.allow")
	opaAdminQuery = prepareAuthzQuery("data.authz.is_admin")
	opaEventQuery = prepareFilterQuery("data.filters.visible_events")
	poolAdmissionQuery, err := rego.New(
		rego.Query("data.pool_admission.allow"),
		rego.Module("pool_admission.rego", poolAdmissionPolicy),
	).PrepareForEval(context.Background())
	if err != nil {
		log.Fatalf("opa: prepare pool admission policy: %v", err)
	}
	opaPoolAdmissionQuery = &poolAdmissionQuery

	// Warm the flag cache once so the first request isn't slowed by the
	// initial resolve and any SSM/provider misconfiguration surfaces in the
	// startup logs rather than on a user's first call.
	_ = flagsData()
}

// EvalPoolAdmission validates an OSGymWorkspacePool write body before it is
// forwarded to Kubernetes.
func EvalPoolAdmission(ctx context.Context, method string, object map[string]any) (bool, error) {
	res, err := opaPoolAdmissionQuery.Eval(ctx, rego.EvalInput(map[string]any{
		"method": method,
		"object": object,
	}))
	if err != nil {
		return false, err
	}
	return res.Allowed(), nil
}

// EvalVisibleEvents runs the data.filters.visible_events query against the
// provided events slice and user sub. Returns the filtered slice.
// items must be a []interface{} (the raw JSON-unmarshalled K8s event objects).
func EvalVisibleEvents(ctx context.Context, user *User, items []interface{}) ([]interface{}, error) {
	input := map[string]interface{}{
		"user":   buildUserInput(user),
		"flags":  flagsData(),
		"events": items,
	}
	res, err := opaEventQuery.Eval(ctx, rego.EvalInput(input))
	if err != nil {
		return nil, err
	}
	if len(res) == 0 || len(res[0].Expressions) == 0 {
		return []interface{}{}, nil
	}
	// visible_events is a set rule — the result is a set of event objects.
	switch v := res[0].Expressions[0].Value.(type) {
	case []interface{}:
		return v, nil
	case map[string]interface{}:
		// OPA may return a set as a map when the set is non-empty.
		out := make([]interface{}, 0, len(v))
		for _, ev := range v {
			out = append(out, ev)
		}
		return out, nil
	}
	return []interface{}{}, nil
}

// EvalIsAdmin returns true if the user's JWT sub is in input.flags.admin_subs
// (data.authz.is_admin). is_admin only reads input.user and input.flags, so
// no route/method/path is needed.
func EvalIsAdmin(ctx context.Context, user *User) (bool, error) {
	if user == nil {
		return false, nil
	}
	input := map[string]interface{}{
		"user":  buildUserInput(user),
		"flags": flagsData(),
	}
	res, err := opaAdminQuery.Eval(ctx, rego.EvalInput(input))
	if err != nil {
		return false, err
	}
	if len(res) == 0 || len(res[0].Expressions) == 0 {
		return false, nil
	}
	v, _ := res[0].Expressions[0].Value.(bool)
	return v, nil
}

func writeJSONErr(w http.ResponseWriter, status int, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]any{"error": msg})
}

// TokenAuthMiddleware validates a Bearer JWT and stores the resulting
// User on the request context. User-key tokens (azp starts with "ukey-")
// are normal JWTs from Keycloak client_credentials grants; they carry
// hardcoded `user_sub` and `user_groups` claims that override the identity
// so the request acts on behalf of the key owner. No `azp` / `namespace`
// enforcement here — that's OPA's job.
func TokenAuthMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		result := "valid"
		defer func() {
			slog.Debug("auth", "elapsed", time.Since(start), "result", result,
				"traceId", r.Context().Value(middlewares.ContextKey("traceId")))
		}()

		raw, err := extractToken(r)
		if err != nil {
			// Fallback: oauth2-proxy sets X-Auth-Request-User when the
			// browser has a valid session cookie. nginx auth_request
			// forwards this header after a successful /oauth2/auth check.
			if proxyUser := r.Header.Get("X-Auth-Request-User"); proxyUser != "" {
				proxyEmail := r.Header.Get("X-Auth-Request-Email")
				user := &User{
					ID:    proxyUser,
					Email: proxyEmail,
					AZP:   "oauth2-proxy", // distinct from SPA/key clients for OPA
				}
				next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), UserKey, user)))
				return
			}
			result = err.Error()
			writeJSONErr(w, http.StatusUnauthorized, "auth token was not provided or is invalid")
			return
		}

		// ── JWT validation ─────────────────────────────────────────
		user, err := validate(raw)
		if err != nil {
			result = err.Error()
			writeJSONErr(w, http.StatusUnauthorized, "auth token is invalid")
			return
		}

		// ── User-key identity override ─────────────────────────────
		// User-key tokens have azp="ukey-..." and carry hardcoded
		// user_sub / user_groups claims from the Keycloak client's
		// protocol mappers. Override the user identity so downstream
		// handlers and OPA see the key owner, not the service account.
		userKeyPfx := "ukey-"
		if authConfig != nil && authConfig.UserKeyClientPfx != "" {
			userKeyPfx = authConfig.UserKeyClientPfx
		}
		if strings.HasPrefix(user.AZP, userKeyPfx) {
			if userSub := user.Claims["user_sub"]; userSub != "" {
				user.ID = userSub
			}
			if userGroups := user.Claims["user_groups"]; userGroups != "" {
				user.Groups = strings.Split(userGroups, ",")
			}
		}

		next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), UserKey, user)))
	})
}

// OpaMiddleware evaluates `data.authz.allow` against
//
//	{ method, path, route, params, user: { sub, azp, namespace, email } }
//
// The `route` and `params` map are filled by RouteContext below before
// this middleware runs; they let policy reason about the matched
// route ("/api/gateway/{name}") and its parameters separately from the
// raw URL path.
func OpaMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		result := "allowed"
		defer func() {
			slog.Debug("opa", "elapsed", time.Since(start), "result", result,
				"traceId", r.Context().Value(middlewares.ContextKey("traceId")))
		}()

		user, _ := r.Context().Value(UserKey).(*User)
		userInput := buildUserInput(user)
		route, _ := r.Context().Value(routeKey).(string)
		params, _ := r.Context().Value(paramsKey).(map[string]string)
		input := map[string]any{
			"method": r.Method,
			"path":   r.URL.Path,
			"route":  route,
			"params": params,
			"user":   userInput,
			"flags":  flagsData(),
		}
		res, err := opaQuery.Eval(r.Context(), rego.EvalInput(input))
		if err != nil {
			result = err.Error()
			writeJSONErr(w, http.StatusInternalServerError, "policy evaluation failed")
			return
		}
		if !res.Allowed() {
			result = "forbidden"
			writeJSONErr(w, http.StatusForbidden, "forbidden")
			return
		}
		next.ServeHTTP(w, r)
	})
}

func buildUserInput(user *User) map[string]any {
	if user == nil {
		return map[string]any{}
	}
	return map[string]any{
		"sub":       user.ID,
		"azp":       user.AZP,
		"namespace": user.Namespace,
		"email":     user.Email,
		"groups":    user.Groups,
	}
}

// route / params context plumbing — keeps the Rego policy stable even if
// the URL pattern changes, and lets it reference `input.params.name`
// directly instead of substring-parsing `input.path`.

type routeCtxKey int

const (
	routeKey  routeCtxKey = 1
	paramsKey routeCtxKey = 2
)

// RouteContext stamps the matched route name + params onto the request
// context before TokenAuthMiddleware/OpaMiddleware run.
func RouteContext(route string, paramKeys ...string) func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			params := map[string]string{}
			for _, k := range paramKeys {
				params[k] = r.PathValue(k)
			}
			ctx := r.Context()
			ctx = context.WithValue(ctx, routeKey, route)
			ctx = context.WithValue(ctx, paramsKey, params)
			next.ServeHTTP(w, r.WithContext(ctx))
		})
	}
}

// GetUser returns the User stamped on the context by TokenAuthMiddleware.
func GetUser(ctx context.Context) *User {
	if u, ok := ctx.Value(UserKey).(*User); ok {
		return u
	}
	return nil
}
