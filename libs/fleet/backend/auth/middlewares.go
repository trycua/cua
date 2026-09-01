package auth

import (
	"context"
	_ "embed"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"log/slog"
	"math"
	"net/http"
	"strings"
	"sync"
	"time"

	"cyclops-cs-backend/metrics"
	"cyclops-cs-backend/middlewares"

	"github.com/open-feature/go-sdk/openfeature"
	"github.com/open-policy-agent/opa/rego"
	"github.com/trycua/cloud/pkg/featureflags"
	"golang.org/x/sync/singleflight"
)

type ctxKey int

const (
	UserKey ctxKey = iota + 1
	freshAdminFlagsKey
)

var (
	opaAdminQuery *rego.PreparedEvalQuery
	opaChatQuery  *rego.PreparedEvalQuery
	opaUsageQuery *rego.PreparedEvalQuery

	opaCardExemptQuery   *rego.PreparedEvalQuery
	opaCardEligibleQuery *rego.PreparedEvalQuery
)

// authzPolicy is the shared principal vocabulary every surface module imports.
// It holds no allow rule of its own; see authz.rego.
//
//go:embed authz.rego
var authzPolicy string

//go:embed pool_admission.rego
var poolAdmissionPolicy string

//go:embed custom_resource_creation_admission.rego
var customResourceCreationAdmissionPolicy string

// authzOwnershipPolicy is the namespace-ownership boundary. Like
// pool_admission.rego it is not a surface — it is a conjunct several surfaces
// carry — so it is registered by name below rather than through
// surfacePolicySources.
//
//go:embed authz_ownership.rego
var authzOwnershipPolicy string

// surfacePolicySources is the base module plus one module per authorization
// surface, keyed by the name policy_routes.go registers it under. Registration
// is a loop over this map rather than a call per module so that adding a surface
// is one entry in one place — and so that a module added here but bound to no
// route is caught by the correspondence test rather than sitting compiled and
// unreachable.
var surfacePolicySources = map[string]struct {
	filename string
	source   string
}{
	"authz-base":                {"authz_base.rego", authzBasePolicy},
	"authz-keys":                {"authz_keys.rego", authzKeysPolicy},
	"authz-config":              {"authz_config.rego", authzConfigPolicy},
	"authz-chat":                {"authz_chat.rego", authzChatPolicy},
	"authz-billing":             {"authz_billing.rego", authzBillingPolicy},
	"authz-usage":               {"authz_usage.rego", authzUsagePolicy},
	"authz-namespaces":          {"authz_namespaces.rego", authzNamespacesPolicy},
	"authz-github-trust":        {"authz_github_trust.rego", authzGitHubTrustPolicy},
	"authz-user-keys":           {"authz_user_keys.rego", authzUserKeysPolicy},
	"authz-k8s":                 {"authz_k8s.rego", authzK8sPolicy},
	"authz-svc":                 {"authz_svc.rego", authzSvcPolicy},
	"authz-signed-service-urls": {"authz_signed_service_urls.rego", authzSignedServiceURLsPolicy},
	"authz-state-query":         {"authz_state_query.rego", authzStateQueryPolicy},
	"authz-feature-flags":       {"authz_feature_flags.rego", authzFeatureFlagsPolicy},
}

//go:embed authz_base.rego
var authzBasePolicy string

//go:embed authz_keys.rego
var authzKeysPolicy string

//go:embed authz_config.rego
var authzConfigPolicy string

//go:embed authz_chat.rego
var authzChatPolicy string

//go:embed authz_billing.rego
var authzBillingPolicy string

//go:embed authz_usage.rego
var authzUsagePolicy string

//go:embed authz_namespaces.rego
var authzNamespacesPolicy string

//go:embed authz_github_trust.rego
var authzGitHubTrustPolicy string

//go:embed authz_user_keys.rego
var authzUserKeysPolicy string

//go:embed authz_k8s.rego
var authzK8sPolicy string

//go:embed authz_svc.rego
var authzSvcPolicy string

//go:embed authz_signed_service_urls.rego
var authzSignedServiceURLsPolicy string

//go:embed authz_state_query.rego
var authzStateQueryPolicy string

// flagPrefix is the OpenFeature path under which the auth package's OPA
// flags live. The dev SimpleEnvProvider maps it to env vars prefixed
// CYCLOPS_CS_ (stripping "/feature-flags/", lowercasing nothing,
// replacing "/" and "-" with "_"); the aws-ssm provider uses the path
// as the SSM Parameter Store prefix directly.
//
//	/feature-flags/cyclops-cs/admin-subs → CYCLOPS_CS_ADMIN_SUBS
//
// OPA list values are JSON string arrays (e.g. '["sub1","sub2"]').
const (
	flagPrefix                    = "/feature-flags/cyclops-cs/"
	adminSubsFlag                 = flagPrefix + "admin-subs"
	billingFlag                   = flagPrefix + "billing-enabled"
	cardAdmissionFlag             = flagPrefix + "require-card-for-custom-resource-creation"
	cardRequirementExemptSubsFlag = flagPrefix + "card-requirement-exempt-subs"
	chatAccessFlag                = flagPrefix + "chat-access"
	chatSubsFlag                  = flagPrefix + "chat-subs"
	usageSubsFlag                 = flagPrefix + "usage-subs"
	usageVCPUHourPriceFlag        = flagPrefix + "usage-vcpu-hour-price-usd"
	usageMemoryGiBHourPriceFlag   = flagPrefix + "usage-memory-gib-hour-price-usd"
)

type chatAccessMode string

const (
	chatAccessDisabled   chatAccessMode = "disabled"
	chatAccessRestricted chatAccessMode = "restricted"
	chatAccessAll        chatAccessMode = "all"
)

// flagsTTL bounds how long flagsData returns its last computed result
// before re-resolving via OpenFeature. Refresh is lazy and ad-hoc: the
// next caller that arrives after expiry triggers a re-resolve on the
// request path — there is no background ticker. Flags are passed into
// each OPA evaluation as `input.flags` (see newRequestPolicyInput /
// EvalIsAdmin), so a refreshed value takes effect on the very next request
// without rebuilding any prepared query or restarting the pod.
const flagsTTL = 1 * time.Minute

var (
	flagsMu         sync.Mutex
	flagsValue      map[string]interface{}
	flagsExp        time.Time
	flagsGeneration uint64

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

	computeFlagsDataFn       = computeFlagsData
	computeFreshAdminFlagsFn = computeFreshAdminFlags
)

// InvalidateFeatureFlags clears the resolved auth flag cache so the next
// authorization request observes a successful feature-flag mutation.
func InvalidateFeatureFlags() {
	flagsMu.Lock()
	flagsValue = nil
	flagsExp = time.Time{}
	flagsGeneration++
	flagsMu.Unlock()
	flagsSF.Forget("flags")
}

// flagsData returns the resolved feature-flag map (admin_subs, …), TTL-cached
// at flagsTTL. On a hit it returns the cached map under the lock; on a miss it
// resolves via OpenFeature behind a singleflight so only one goroutine fans out
// to SSM while the rest share the result. The map is meant to be placed under
// input.flags for OPA; the policy references a subset of keys (admin_subs),
// extras are ignored, and absent keys evaluate as undefined (implicitly
// denying).
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
		generation := flagsGeneration
		flagsMu.Unlock()

		// Resolve with a detached context so a single caller's cancelled
		// request can't abort the refresh shared by everyone else.
		result := computeFlagsDataFn(context.Background())

		flagsMu.Lock()
		if flagsGeneration == generation {
			flagsValue = result
			flagsExp = time.Now().Add(flagsTTL)
		}
		flagsMu.Unlock()
		return result, nil
	})
	return v.(map[string]interface{})
}

func computeFreshAdminFlags(ctx context.Context) map[string]interface{} {
	ffClientOnce.Do(func() {
		ffClient = openfeature.NewClient("cyclops-cs-auth")
	})
	key := flagPrefix + "admin-subs"
	admins, err := loadStringList(ctx, ffClient, key)
	if err != nil {
		slog.Warn("auth: fresh admin flag load failed; denying admin access", "flag", key, "err", err)
		admins = []interface{}{}
	}
	return map[string]interface{}{
		"admin_subs": admins,
	}
}

func freshAdminFlags(ctx context.Context) map[string]interface{} {
	if flags, ok := ctx.Value(freshAdminFlagsKey).(map[string]interface{}); ok {
		return flags
	}
	return computeFreshAdminFlagsFn(ctx)
}

func withFreshAdminFlags(request *http.Request) *http.Request {
	if _, ok := request.Context().Value(freshAdminFlagsKey).(map[string]interface{}); ok {
		return request
	}
	flags := computeFreshAdminFlagsFn(request.Context())
	return request.WithContext(context.WithValue(request.Context(), freshAdminFlagsKey, flags))
}

func authorizationFlags(ctx context.Context) map[string]interface{} {
	if fresh, ok := ctx.Value(freshAdminFlagsKey).(map[string]interface{}); ok {
		cached := flagsData()
		flags := make(map[string]interface{}, len(cached)+1)
		for key, value := range cached {
			flags[key] = value
		}
		flags["admin_subs"] = fresh["admin_subs"]
		return flags
	}
	return flagsData()
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
		if !isOPAStringListFlag(key) {
			continue
		}
		name := opaNameFromFlagKey(key)
		if name == "" {
			continue
		}
		items, err := loadStringList(ctx, ffClient, key)
		if err != nil {
			slog.Warn("auth: flag load failed; flags will be empty", "flag", key, "err", err)
			return map[string]interface{}{}
		}
		flags[name] = items
	}
	callCtx, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()
	enabled, err := ffClient.BooleanValue(
		callCtx, cardAdmissionFlag, false, openfeature.EvaluationContext{},
	)
	if err != nil {
		slog.Warn("auth: flag eval failed; using false", "flag", cardAdmissionFlag, "err", err)
	}
	flags["require_card_for_custom_resource_creation"] = enabled
	return flags
}

func isOPAStringListFlag(key string) bool {
	switch key {
	case adminSubsFlag, cardRequirementExemptSubsFlag, chatSubsFlag, usageSubsFlag:
		return true
	default:
		return false
	}
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
// input.flags document. Evaluation and decoding failures are returned so
// computeFlagsData can discard the entire authorization flag set.
//
// A 3s per-call deadline keeps an unreachable Parameter Store from
// stalling pod startup; the from-env fallback kicks in on timeout.
func loadStringList(ctx context.Context, client *openfeature.Client, flagKey string) ([]interface{}, error) {
	callCtx, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()
	raw, err := client.StringValue(callCtx, flagKey, "[]", openfeature.EvaluationContext{})
	if err != nil {
		return nil, fmt.Errorf("evaluate %s as string list: %w", flagKey, err)
	}
	var items []string
	if err := json.Unmarshal([]byte(raw), &items); err != nil {
		return nil, fmt.Errorf("decode %s as JSON string array: %w", flagKey, err)
	}
	out := make([]interface{}, len(items))
	for i, v := range items {
		out[i] = v
	}
	return out, nil
}

// LoadOpa prepares the embedded Rego policy and all derived queries.
// Feature flags are NOT baked into an OPA data store here; they are passed
// per-request as input.flags (see flagsData), so prepared queries stay
// valid across flag refreshes. The OpenFeature provider must be installed
// by featureflags.SetupProvider before this runs.
func LoadOpa() {
	RegisterPolicyModule("authz", "authz.rego", authzPolicy)
	RegisterPolicyModule("pool-admission", "pool_admission.rego", poolAdmissionPolicy)
	RegisterPolicyModule("custom-resource-creation-admission", "custom_resource_creation_admission.rego", customResourceCreationAdmissionPolicy)
	RegisterPolicyModule("authz-ownership", "authz_ownership.rego", authzOwnershipPolicy)
	for name, module := range surfacePolicySources {
		RegisterPolicyModule(name, module.filename, module.source)
	}

	// authz queries only need authz.rego. Route authorization is not one of
	// them — that runs through the policy library, which composes the base
	// module with one surface module per route from the registry above. What is
	// left is is_admin, a decision EvalIsAdmin makes off a User rather than a
	// request, which is why it is not a policy-library node.
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

	opaAdminQuery = prepareAuthzQuery("data.authz.is_admin")
	opaChatQuery = prepareAuthzQuery("data.authz.chat_enabled")
	opaUsageQuery = prepareAuthzQuery("data.authz.usage_enabled")

	// Card-admission queries pair authz.rego with the admission module because
	// custom_resource_creation_admission.rego imports data.authz for is_admin.
	prepareCardAdmissionQuery := func(q string) *rego.PreparedEvalQuery {
		pq, err := rego.New(
			rego.Query(q),
			rego.Module("authz.rego", authzPolicy),
			rego.Module("custom_resource_creation_admission.rego", customResourceCreationAdmissionPolicy),
		).PrepareForEval(context.Background())
		if err != nil {
			log.Fatalf("opa: prepare card admission %q: %v", q, err)
		}
		return &pq
	}
	opaCardExemptQuery = prepareCardAdmissionQuery("data.custom_resource_creation_admission.exempt")
	opaCardEligibleQuery = prepareCardAdmissionQuery("data.custom_resource_creation_admission.billing_eligible")

	// Warm the flag cache once so the first request isn't slowed by the
	// initial resolve and any SSM/provider misconfiguration surfaces in the
	// startup logs rather than on a user's first call.
	_ = flagsData()
}

// EvalIsAdmin returns true if the user's JWT sub is in input.flags.admin_subs
// (data.authz.is_admin). is_admin only reads input.user and input.flags, so
// no route/method/path is needed.
func EvalIsAdmin(ctx context.Context, user *User) (bool, error) {
	return evalUserDecision(ctx, user, opaAdminQuery, flagsData())
}

// EvalIsAdminFresh bypasses the process-local admin membership cache. If the
// feature-admin middleware already resolved membership for this request, the
// handler reuses that result instead of issuing a duplicate provider call.
func EvalIsAdminFresh(ctx context.Context, user *User) (bool, error) {
	return evalUserDecision(ctx, user, opaAdminQuery, freshAdminFlags(ctx))
}

// EvalChatEnabled resolves the global chat access mode and applies the
// restricted-mode administrator/chat-subs policy when needed. Missing,
// malformed, or unsupported values default to restricted.
func EvalChatEnabled(ctx context.Context, user *User) (bool, error) {
	if user == nil || user.ID == "" {
		return false, nil
	}
	switch resolveChatAccessMode(ctx, user.ID) {
	case chatAccessDisabled:
		return false, nil
	case chatAccessAll:
		return true, nil
	default:
		return evalUserDecision(ctx, user, opaChatQuery, flagsData())
	}
}

func resolveChatAccessMode(ctx context.Context, targetingKey string) chatAccessMode {
	ffClientOnce.Do(func() {
		ffClient = openfeature.NewClient("cyclops-cs-auth")
	})
	callCtx, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()
	raw, err := ffClient.StringValue(
		callCtx,
		chatAccessFlag,
		string(chatAccessRestricted),
		openfeature.NewEvaluationContext(targetingKey, nil),
	)
	if err != nil {
		slog.WarnContext(ctx, "auth: chat access flag eval failed; using restricted", "flag", chatAccessFlag, "err", err)
		return chatAccessRestricted
	}
	switch chatAccessMode(strings.ToLower(strings.TrimSpace(raw))) {
	case chatAccessDisabled:
		return chatAccessDisabled
	case chatAccessRestricted:
		return chatAccessRestricted
	case chatAccessAll:
		return chatAccessAll
	default:
		slog.WarnContext(ctx, "auth: invalid chat access flag; using restricted", "flag", chatAccessFlag, "value", raw)
		return chatAccessRestricted
	}
}

func EvalUsageEnabled(ctx context.Context, user *User) (bool, error) {
	return evalUserDecision(ctx, user, opaUsageQuery, flagsData())
}

func evalUserDecision(ctx context.Context, user *User, query *rego.PreparedEvalQuery, flags map[string]interface{}) (bool, error) {
	if user == nil {
		return false, nil
	}
	input := map[string]interface{}{
		"user":  buildUserInput(user),
		"flags": flags,
	}
	res, err := query.Eval(ctx, rego.EvalInput(input))
	if err != nil {
		return false, err
	}
	if len(res) == 0 || len(res[0].Expressions) == 0 {
		return false, nil
	}
	v, _ := res[0].Expressions[0].Value.(bool)
	return v, nil
}

const (
	DefaultUsageVCPUHourPriceUSD      = 0.044625
	DefaultUsageMemoryGiBHourPriceUSD = 0.0223125
)

type UsagePricing struct {
	VCPUHourUSD      float64
	MemoryGiBHourUSD float64
}

// EvalUsagePricing resolves billable reservation rates. Invalid or missing
// values fall back independently so one malformed flag cannot zero all costs.
func EvalUsagePricing(ctx context.Context, user *User) (UsagePricing, error) {
	ffClientOnce.Do(func() {
		ffClient = openfeature.NewClient("cyclops-cs-auth")
	})
	targetingKey := ""
	if user != nil {
		targetingKey = user.ID
	}
	evaluationContext := openfeature.NewEvaluationContext(targetingKey, nil)
	callCtx, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()

	vcpu, vcpuErr := ffClient.FloatValue(callCtx, usageVCPUHourPriceFlag, DefaultUsageVCPUHourPriceUSD, evaluationContext)
	if vcpuErr != nil {
		vcpu = DefaultUsageVCPUHourPriceUSD
	} else if math.IsNaN(vcpu) || math.IsInf(vcpu, 0) || vcpu <= 0 {
		vcpuErr = fmt.Errorf("usage vCPU-hour price must be a positive finite number")
		vcpu = DefaultUsageVCPUHourPriceUSD
	}
	memory, memoryErr := ffClient.FloatValue(callCtx, usageMemoryGiBHourPriceFlag, DefaultUsageMemoryGiBHourPriceUSD, evaluationContext)
	if memoryErr != nil {
		memory = DefaultUsageMemoryGiBHourPriceUSD
	} else if math.IsNaN(memory) || math.IsInf(memory, 0) || memory <= 0 {
		memoryErr = fmt.Errorf("usage memory GiB-hour price must be a positive finite number")
		memory = DefaultUsageMemoryGiBHourPriceUSD
	}
	return UsagePricing{VCPUHourUSD: vcpu, MemoryGiBHourUSD: memory}, errors.Join(vcpuErr, memoryErr)
}

// EvalBillingEnabled resolves the billing UI flag for the authenticated user.
// The flag defaults off so absent provider configuration never exposes billing.
func EvalBillingEnabled(ctx context.Context, user *User) (bool, error) {
	if user == nil || user.ID == "" {
		return false, nil
	}
	ffClientOnce.Do(func() {
		ffClient = openfeature.NewClient("cyclops-cs-auth")
	})
	callCtx, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()
	return ffClient.BooleanValue(callCtx, billingFlag, false, openfeature.NewEvaluationContext(user.ID, nil))
}

// cardAdmissionProbePath is a representative custom-resource create path so
// the admission module's is_custom_resource_create matches; without it the
// "not is_custom_resource_create" rule would exempt everyone.
const cardAdmissionProbePath = "apis/cua.ai/v1/namespaces/probe/osgymsandboxwarmpools"

func cardAdmissionInput(user *User, stripeCards FactSet, now time.Time) map[string]interface{} {
	input := map[string]interface{}{
		"user":   buildUserInput(user),
		"flags":  flagsData(),
		"method": http.MethodPost,
		"params": map[string]interface{}{"path": cardAdmissionProbePath},
	}
	if stripeCards != nil {
		input["facts"] = map[string]interface{}{
			StripeCardsFactNamespace: map[string]any(stripeCards),
			TimeFactNamespace: map[string]interface{}{
				"current_year":  now.UTC().Year(),
				"current_month": int(now.UTC().Month()),
			},
		}
	}
	return input
}

func evalPreparedBool(ctx context.Context, query *rego.PreparedEvalQuery, input map[string]interface{}) (bool, error) {
	if query == nil {
		return false, fmt.Errorf("opa card admission query is not prepared; call LoadOpa first")
	}
	res, err := query.Eval(ctx, rego.EvalInput(input))
	if err != nil {
		return false, err
	}
	if len(res) == 0 || len(res[0].Expressions) == 0 {
		return false, nil
	}
	v, _ := res[0].Expressions[0].Value.(bool)
	return v, nil
}

// EvalPoolCreateCardRequired reports whether the card-admission policy would
// deny this user a custom-resource create (a pool, template, or claim) for
// lack of a qualifying payment card. It evaluates the same Rego module the
// /api/k8s route enforces, against a representative create request, so the
// advisory answer the dashboard shows cannot drift from enforcement.
// loadStripeCards is called only when the user is not otherwise exempt
// (flag off, admin, exempt sub), mirroring the enforcement path's lazy fact
// loading; it should return the StripeCardsFactNamespace fact set.
func EvalPoolCreateCardRequired(
	ctx context.Context,
	user *User,
	loadStripeCards func(context.Context) (FactSet, error),
) (bool, error) {
	if user == nil || user.ID == "" {
		return false, nil
	}
	// The exempt rules read only input.flags and input.user, so no facts
	// (and no clock) are attached to this first evaluation.
	exempt, err := evalPreparedBool(ctx, opaCardExemptQuery, cardAdmissionInput(user, nil, time.Time{}))
	if err != nil {
		return false, err
	}
	if exempt {
		return false, nil
	}
	stripeCards, err := loadStripeCards(ctx)
	if err != nil {
		return false, err
	}
	eligible, err := evalPreparedBool(ctx, opaCardEligibleQuery, cardAdmissionInput(user, stripeCards, time.Now()))
	if err != nil {
		return false, err
	}
	return !eligible, nil
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
					ID:            proxyUser,
					Email:         proxyEmail,
					EmailVerified: proxyEmail != "",
					AZP:           "oauth2-proxy", // distinct from SPA/key clients for OPA
					KeyClientPfx:  configuredKeyClientPfx(),
				}
				metrics.SetRequestUser(r.Context(), user.ID)
				next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), UserKey, user)))
				return
			}
			result = err.Error()
			writeJSONErr(w, http.StatusUnauthorized, "auth token was not provided or is invalid")
			return
		}

		// ── JWT validation ─────────────────────────────────────────
		user, err := validateWithContext(r.Context(), raw)
		if err != nil {
			result = err.Error()
			if IsDatabaseUnavailable(err) {
				writeJSONErr(w, http.StatusServiceUnavailable, "authentication unavailable")
				return
			}
			writeJSONErr(w, http.StatusUnauthorized, "auth token is invalid")
			return
		}
		user.KeyClientPfx = configuredKeyClientPfx()

		// ── User-key identity override ─────────────────────────────
		// User-key tokens have azp="ukey-..." and carry hardcoded
		// user_sub / user_groups claims from the Keycloak client's
		// protocol mappers. Override the user identity so downstream
		// handlers and OPA see the key owner, not the service account.
		userKeyPfx := "ukey-"
		if authConfig != nil && authConfig.UserKeyClientPfx != "" {
			userKeyPfx = authConfig.UserKeyClientPfx
		}
		if err := applyUserKeyIdentity(user, userKeyPfx); err != nil {
			result = err.Error()
			writeJSONErr(w, http.StatusUnauthorized, "auth token is invalid")
			return
		}

		metrics.SetRequestUser(r.Context(), user.ID)
		next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), UserKey, user)))
	})
}

func configuredKeyClientPfx() string {
	if authConfig != nil && authConfig.KeyClientPfx != "" {
		return authConfig.KeyClientPfx
	}
	return "key-"
}

func keyClientPfx(value string) string {
	if value == "" {
		return "key-"
	}
	return value
}

func applyUserKeyIdentity(user *User, userKeyPfx string) error {
	if !strings.HasPrefix(user.AZP, userKeyPfx) {
		return nil
	}
	userSub := user.Claims["user_sub"]
	if userSub == "" {
		return fmt.Errorf("user-key token is missing user_sub")
	}
	user.ID = userSub
	user.PrincipalType = PrincipalTypeUserKey
	// Ignore any service-account email claims. Only the creation-time owner
	// evidence stamped by the backend may classify a user-key request.
	user.Email = strings.TrimSpace(user.Claims["user_email"])
	user.EmailVerified = user.Email != "" && user.Claims["user_email_verified"] == "true"
	if userGroups := user.Claims["user_groups"]; userGroups != "" {
		user.Groups = strings.Split(userGroups, ",")
	}
	return nil
}

func buildUserInput(user *User) map[string]any {
	if user == nil {
		return map[string]any{}
	}
	return map[string]any{
		"sub":                user.ID,
		"azp":                user.AZP,
		"key_client_prefix":  keyClientPfx(user.KeyClientPfx),
		"namespace":          user.Namespace,
		"email":              user.Email,
		"groups":             user.Groups,
		"principal_type":     user.PrincipalType,
		"repository":         user.Repository,
		"allowed_namespaces": user.AllowedNamespaces,
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

// routeParamNames returns the wildcard names in a route pattern, in the order
// they appear: "/api/svc/{namespace}/{service}/{path...}" yields namespace,
// service, path. These are the names r.PathValue is keyed by, so the trailing
// "..." of a multi-segment wildcard is not part of the name.
//
// This scans the Go 1.22 ServeMux pattern grammar and nothing wider: a wildcard
// is a whole path segment wrapped in braces, and "{$}" — the end-of-path anchor
// — binds no value.
func routeParamNames(route string) []string {
	var names []string
	for _, segment := range strings.Split(route, "/") {
		if len(segment) < 2 || segment[0] != '{' || segment[len(segment)-1] != '}' {
			continue
		}
		name := strings.TrimSuffix(segment[1:len(segment)-1], "...")
		if name == "" || name == "$" {
			continue
		}
		names = append(names, name)
	}
	return names
}

// RouteContext stamps the matched route name + params onto the request
// context before TokenAuthMiddleware and the authorization stage run.
//
// The parameter names come from the route pattern rather than from a list the
// caller supplies alongside it. A caller-supplied list is a second copy of what
// the pattern already says, and the two can disagree silently: drop a name and
// the policy's input.params.X is undefined, its rule body fails, `default allow
// = false` answers, and the route returns 403 for every request — no compile
// error, no log, and the cause is a deleted string in a different file and a
// different language from the policy that needed it. Deriving them means there
// is no second list to disagree with.
func RouteContext(route string) func(http.Handler) http.Handler {
	paramKeys := routeParamNames(route)
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

//go:embed authz_feature_flags.rego
var authzFeatureFlagsPolicy string
