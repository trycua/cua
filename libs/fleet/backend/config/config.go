// Package config loads cyclops-cs backend config via Cobra + Viper.
//
// Precedence: command-line flag > environment variable > hardcoded default.
// Existing env var names (KC_BASE_URL, LISTEN_ADDR, ORCH_*, …) are preserved
// so no deployment/Helm change is required. Defaults equal the previous
// hardcoded literals, so behaviour is unchanged unless explicitly overridden.
//
// Cross-service identity prefixes (user-/oidc:) are NOT here — they live in
// OpenFeature/SSM so backend impersonation and the standalone Tenant controller
// use the same values (see package identity). The key-/ukey- client-id prefixes are
// cyclops-only, so they stay in this local config.
package config

import (
	"crypto/sha256"
	"fmt"
	"net/url"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/spf13/pflag"
	"github.com/spf13/viper"
)

type Configuration struct {
	WebServer        WebServerConfiguration
	Auth             AuthConfiguration
	Keycloak         KeycloakConfiguration
	Gateway          GatewayConfiguration
	Database         DatabaseConfiguration
	Stripe           StripeConfiguration
	Chat             ChatConfiguration
	Metrics          MetricsConfiguration
	Telemetry        TelemetryConfiguration
	Usage            UsageConfiguration
	ProductAnalytics ProductAnalyticsConfiguration
	SignedServiceURL SignedServiceURLConfiguration
}

type WebServerConfiguration struct {
	Addr string
}

// AuthConfiguration follows the grt naming so the JWKS / verifier code
// reads like the upstream template.
type AuthConfiguration struct {
	Issuer                    string   // https://auth.cua.ai/realms/cyclops-cs
	JWKSUri                   string   // <Issuer>/protocol/openid-connect/certs
	SigningAlgs               []string // RS256, RS512, ES256
	SPAClientID               string
	KeyClientPfx              string // pool/gateway key client-id prefix ("key-")
	UserKeyClientPfx          string // per-user key client-id prefix ("ukey-")
	GitHubOIDCEnabled         bool
	GitHubOIDCIssuer          string
	GitHubOIDCJWKSUri         string
	GitHubOIDCAudience        string
	GitHubOIDCLegacyAudiences []string
	GitHubOIDCAlgs            []string
}

type KeycloakConfiguration struct {
	BaseURL           string
	Realm             string
	AdminClientID     string
	AdminClientSecret string
	TokenURL          string

	// Workloads realm — the machine-only realm AWS STS (and other OIDC-trust
	// providers) federate against. The backend mints a per-tenant
	// service-account client here so OSGym pool VMs can obtain a
	// tenant-scoped OIDC token. Disabled (no client minted) when
	// WorkloadAdminClientSecret is empty. The admin client lives IN the
	// workloads realm (Keycloak realm-management roles are per-realm, so the
	// cyclops-cs-realm admin can't manage workloads). See
	// docs/decisions/2026-06-25-osgym-pool-workload-oidc.md.
	WorkloadRealm             string
	WorkloadAdminClientID     string
	WorkloadAdminClientSecret string
	WorkloadTokenURL          string
	WorkloadAudience          string
}

type GatewayConfiguration struct {
	Scheme        string
	Port          string
	ClusterDomain string
}

type StripeConfiguration struct {
	SecretKey          string
	WebhookSecret      string
	CheckoutSuccessURL string
	CheckoutCancelURL  string
	PortalReturnURL    string
}

// DatabaseConfiguration drives the Postgres-backed stores, including GitHub
// OIDC trust policies and signed service URLs. An empty URL disables those routes (503),
// keeping the backend bootable without a database — see CUA-675.
type DatabaseConfiguration struct {
	URL                      string // DATABASE_URL — application database and GitHub trust-policy storage
	StateQueryDSN            string // STATE_QUERY_DATABASE_DSN — tenant query connection options
	StateQueryTenantPassword string // STATE_QUERY_TENANT_PASSWORD — shared tenant query login password
}

type ChatConfiguration struct {
	BaseURL string
	APIKey  string
	Model   string
}

// SignedServiceURLConfiguration enables database-backed signed capability
// URLs. A base URL and HMAC secret from either the environment or SecretFile
// are required; leaving every value empty disables only this feature so
// deployments can roll out its Secret independently.
type SignedServiceURLConfiguration struct {
	BaseURL    string // SIGNED_SERVICE_URL_BASE_URL — trusted external HTTPS origin
	Secret     string // SIGNED_SERVICE_URL_SECRET — HMAC key, never logged
	SecretFile string // SIGNED_SERVICE_URL_SECRET_FILE — optional path to the HMAC key
}

// UsageConfiguration enables the usage provider only when DatabaseURL is set.
// Query transport settings may be present before the usage credential arrives;
// in that state the provider remains disabled so serving readiness is unaffected.
type UsageConfiguration struct {
	DatabaseURL       string
	QueryWebhookURL   string
	QueryHMACSecret   string
	QueryResultBucket string
	QueryResultPrefix string
	QueryCluster      string
	QueryEnvironment  string
	QueryTimeout      time.Duration
	QueryPollInterval time.Duration
	MaxResponseBytes  int64
}

var usageQueryPartitionValue = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]*$`)

const (
	clusterUsageQueryWebhookURL  = "http://cua-temporal-webhook.temporal.svc.cluster.local/hooks/opencost-query"
	externalUsageQueryWebhookURL = "https://cua-temporal-webhook.tail204509.ts.net/hooks/opencost-query"
	minUsageQueryTimeout         = time.Second
	maxUsageQueryTimeout         = 2 * time.Minute
	minUsageResponseSize         = 64 * 1024
	maxUsageResponseSize         = 32 * 1024 * 1024
)

type MetricsConfiguration struct {
	Addr string // METRICS_ADDR — Prometheus listen addr
}

type ProductAnalyticsConfiguration struct {
	Enabled          bool
	Host             string
	ProjectToken     string
	IdentityKey      string
	Environment      string
	ExcludedSubjects []string
}

type TelemetryConfiguration struct {
	Endpoint         string // OTEL_EXPORTER_OTLP_ENDPOINT
	Protocol         string // OTEL_EXPORTER_OTLP_PROTOCOL
	ServiceName      string // OTEL_SERVICE_NAME
	ServiceNamespace string // OTEL_SERVICE_NAMESPACE
	Environment      string // OTEL_ENVIRONMENT
	ResourceAttrs    string // OTEL_RESOURCE_ATTRIBUTES
}

// flagSpec maps one config value to its viper key, CLI flag, env var, and
// hardcoded default. Keeping them in one table guarantees flag/env/default
// stay in sync.
type flagSpec struct {
	key, flag, env, def, usage string
}

var specs = []flagSpec{
	{"webserver.addr", "listen-addr", "LISTEN_ADDR", "127.0.0.1:8080", "HTTP listen address"},
	{"kc.base-url", "kc-base-url", "KC_BASE_URL", "https://auth.cua.ai", "Keycloak base URL"},
	{"kc.realm", "kc-realm", "KC_REALM", "cyclops-cs", "Keycloak realm"},
	{"kc.issuer", "kc-issuer", "KC_ISSUER", "", "JWT issuer override (defaults to <base>/realms/<realm>)"},
	{"kc.spa-client-id", "kc-spa-client-id", "KC_SPA_CLIENT_ID", "cyclops-cs-spa", "SPA OIDC client id"},
	{"kc.key-client-prefix", "kc-key-client-prefix", "KC_KEY_CLIENT_PFX", "key-", "pool/gateway key client-id prefix"},
	{"kc.user-key-client-prefix", "kc-user-key-client-prefix", "KC_USER_KEY_CLIENT_PFX", "ukey-", "per-user key client-id prefix"},
	{"github.oidc-issuer", "github-oidc-issuer", "GITHUB_OIDC_ISSUER", "https://token.actions.githubusercontent.com", "GitHub Actions OIDC issuer"},
	{"github.oidc-jwks-uri", "github-oidc-jwks-uri", "GITHUB_OIDC_JWKS_URI", "https://token.actions.githubusercontent.com/.well-known/jwks", "GitHub Actions OIDC JWKS URI"},
	{"github.oidc-audience", "github-oidc-audience", "GITHUB_OIDC_AUDIENCE", "fleets", "Audience required on inbound GitHub OIDC tokens"},
	{"github.oidc-legacy-audiences", "github-oidc-legacy-audiences", "GITHUB_OIDC_LEGACY_AUDIENCES", "cyclops-cs", "Comma-separated legacy audiences accepted on inbound GitHub OIDC tokens"},
	{"kc.admin-client-id", "kc-admin-client-id", "KC_ADMIN_CLIENT_ID", "cyclops-cs-backend", "Keycloak admin client id"},
	{"kc.admin-client-secret", "kc-admin-client-secret", "KC_ADMIN_CLIENT_SECRET", "", "Keycloak admin client secret (required)"},
	{"kc.workload-realm", "kc-workload-realm", "KC_WORKLOAD_REALM", "workloads", "Keycloak realm AWS/OIDC trusts for pool VM tokens"},
	{"kc.workload-admin-client-id", "kc-workload-admin-client-id", "KC_WORKLOAD_ADMIN_CLIENT_ID", "workloads-admin", "admin client id IN the workloads realm"},
	{"kc.workload-admin-client-secret", "kc-workload-admin-client-secret", "KC_WORKLOAD_ADMIN_CLIENT_SECRET", "", "admin client secret for the workloads realm (enables per-tenant pool VM OIDC when set)"},
	{"kc.workload-audience", "kc-workload-audience", "KC_WORKLOAD_AUDIENCE", "sts.amazonaws.com", "aud claim stamped on per-tenant pool VM workload tokens"},
	{"gateway.scheme", "orch-scheme", "ORCH_SCHEME", "http", "orchestrator scheme"},
	{"gateway.port", "orch-port", "ORCH_PORT", "80", "orchestrator port"},
	{"gateway.cluster-domain", "cluster-domain", "CLUSTER_DOMAIN", "svc.cluster.local", "in-cluster DNS domain"},
	{"database.url", "database-url", "DATABASE_URL", "", "Postgres URL for trust-policy storage (enables /api/github-trust-policies)"},
	{"database.state-query-dsn", "state-query-database-dsn", "STATE_QUERY_DATABASE_DSN", "", "Postgres connection options for tenant state queries"},
	{"database.state-query-tenant-password", "state-query-tenant-password", "STATE_QUERY_TENANT_PASSWORD", "", "Shared password for tenant query login roles"},
	{"usage.database-url", "usage-database-url", "USAGE_DATABASE_URL", "", "Postgres URL for usage events"},
	{"usage.query-webhook-url", "usage-query-webhook-url", "USAGE_QUERY_WEBHOOK_URL", clusterUsageQueryWebhookURL, "DataFusion allocation query webhook URL"},
	{"usage.query-hmac-secret", "usage-query-hmac-secret", "USAGE_QUERY_HMAC_SECRET", "", "DataFusion allocation query webhook HMAC secret"},
	{"usage.query-result-bucket", "usage-query-result-bucket", "USAGE_QUERY_RESULT_BUCKET", "nanoclaw-telemetry-files", "S3 bucket for transient allocation query results"},
	{"usage.query-result-prefix", "usage-query-result-prefix", "USAGE_QUERY_RESULT_PREFIX", "cyclops/usage-query", "S3 prefix for transient allocation query results"},
	{"usage.query-cluster", "usage-query-cluster", "USAGE_QUERY_CLUSTER", "kopf-k3s", "OpenCost allocation cluster partition"},
	{"usage.query-environment", "usage-query-environment", "USAGE_QUERY_ENVIRONMENT", "production", "OpenCost allocation environment partition"},
	{"usage.query-timeout", "usage-query-timeout", "USAGE_QUERY_TIMEOUT", "45s", "Usage provider query timeout"},
	{"usage.query-poll-interval", "usage-query-poll-interval", "USAGE_QUERY_POLL_INTERVAL", "1s", "Allocation query status poll interval"},
	{"usage.max-response-bytes", "usage-max-response-bytes", "USAGE_MAX_RESPONSE_BYTES", "8388608", "Maximum allocation query response bytes"},
	{"stripe.secret-key", "stripe-secret-key", "STRIPE_SECRET_KEY", "", "Stripe secret key (server-only)"},
	{"stripe.webhook-secret", "stripe-webhook-secret", "STRIPE_WEBHOOK_SECRET", "", "Stripe webhook signing secret"},
	{"stripe.checkout-success-url", "stripe-checkout-success-url", "STRIPE_CHECKOUT_SUCCESS_URL", "", "Stripe Checkout success redirect URL"},
	{"stripe.checkout-cancel-url", "stripe-checkout-cancel-url", "STRIPE_CHECKOUT_CANCEL_URL", "", "Stripe Checkout cancel redirect URL"},
	{"stripe.portal-return-url", "stripe-portal-return-url", "STRIPE_PORTAL_RETURN_URL", "", "Stripe Billing Portal return URL"},
	{"signed-service-url.base-url", "signed-service-url-base-url", "SIGNED_SERVICE_URL_BASE_URL", "", "Trusted external HTTPS origin for signed sandbox service URLs"},
	{"signed-service-url.secret", "signed-service-url-secret", "SIGNED_SERVICE_URL_SECRET", "", "HMAC secret for signed sandbox service URLs"},
	{"signed-service-url.secret-file", "signed-service-url-secret-file", "SIGNED_SERVICE_URL_SECRET_FILE", "", "Optional path to the signed sandbox service URL HMAC secret"},
	{"chat.base-url", "litellm-base-url", "LITELLM_BASE_URL", "", "LiteLLM OpenAI-compatible base URL"},
	{"chat.api-key", "litellm-api-key", "LITELLM_API_KEY", "", "LiteLLM virtual key"},
	{"chat.model", "litellm-model", "LITELLM_MODEL", "large", "LiteLLM model alias"},
	{"metrics.addr", "metrics-addr", "METRICS_ADDR", ":9091", "Prometheus metrics listen address"},
	{"product-analytics.enabled", "fleet-analytics-enabled", "FLEET_ANALYTICS_ENABLED", "false", "Enable Fleet product analytics delivery"},
	{"product-analytics.host", "posthog-host", "POSTHOG_HOST", "https://eu.i.posthog.com", "PostHog capture host"},
	{"product-analytics.project-token", "posthog-project-token", "POSTHOG_PROJECT_TOKEN", "", "PostHog project token"},
	{"product-analytics.identity-key", "posthog-identity-key", "POSTHOG_IDENTITY_KEY", "", "Key for pseudonymous product analytics identity"},
	{"product-analytics.excluded-subjects", "fleet-analytics-excluded-subs", "FLEET_ANALYTICS_EXCLUDED_SUBS", "", "Comma-separated Keycloak subjects excluded from analytics"},
	{"telemetry.endpoint", "otel-endpoint", "OTEL_EXPORTER_OTLP_ENDPOINT", "https://otel.cua.ai", "OTLP HTTP traces endpoint"},
	{"telemetry.protocol", "otel-protocol", "OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf", "OTLP exporter protocol"},
	{"telemetry.service-name", "otel-service-name", "OTEL_SERVICE_NAME", "cyclops-cs-backend", "OTEL service.name"},
	{"telemetry.service-namespace", "otel-service-namespace", "OTEL_SERVICE_NAMESPACE", "cyclops-cs", "OTEL service.namespace"},
	{"telemetry.environment", "otel-environment", "OTEL_ENVIRONMENT", "production", "OTEL deployment environment"},
	{"telemetry.resource-attributes", "otel-resource-attributes", "OTEL_RESOURCE_ATTRIBUTES", "", "Additional OTEL resource attributes"},
}

// RegisterFlags wires the flag set + viper bindings. Call once from the Cobra
// rootCmd before LoadConfig (Cobra parses flags before RunE runs).
func RegisterFlags(fs *pflag.FlagSet) {
	for _, s := range specs {
		fs.String(s.flag, s.def, s.usage)
		_ = viper.BindPFlag(s.key, fs.Lookup(s.flag))
		_ = viper.BindEnv(s.key, s.env)
		viper.SetDefault(s.key, s.def)
	}
}

func splitCommaSeparated(value string) []string {
	values := strings.FieldsFunc(value, func(r rune) bool { return r == ',' })
	out := make([]string, 0, len(values))
	for _, value := range values {
		value = strings.TrimSpace(value)
		if value != "" && !slices.Contains(out, value) {
			out = append(out, value)
		}
	}
	return out
}

func LoadConfig() (*Configuration, error) {
	usageQueryTimeout, err := parseUsageQueryTimeout(viper.GetString("usage.query-timeout"))
	if err != nil {
		return nil, err
	}
	usagePollInterval, err := parseUsagePollInterval(viper.GetString("usage.query-poll-interval"))
	if err != nil {
		return nil, err
	}
	usageMaxResponseBytes, err := parseUsageMaxResponseBytes(viper.GetString("usage.max-response-bytes"))
	if err != nil {
		return nil, err
	}

	base := strings.TrimRight(viper.GetString("kc.base-url"), "/")
	realm := viper.GetString("kc.realm")
	realmPath := fmt.Sprintf("%s/realms/%s", base, realm)

	// KC_ISSUER overrides the expected `iss` claim; default to the realm path.
	issuer := viper.GetString("kc.issuer")
	if issuer == "" {
		issuer = realmPath
	}

	cfg := &Configuration{
		WebServer: WebServerConfiguration{Addr: viper.GetString("webserver.addr")},
		Auth: AuthConfiguration{
			Issuer:             issuer,
			JWKSUri:            realmPath + "/protocol/openid-connect/certs",
			SigningAlgs:        []string{"RS256", "RS512", "ES256"},
			SPAClientID:        viper.GetString("kc.spa-client-id"),
			KeyClientPfx:       viper.GetString("kc.key-client-prefix"),
			UserKeyClientPfx:   viper.GetString("kc.user-key-client-prefix"),
			GitHubOIDCEnabled:  true,
			GitHubOIDCIssuer:   viper.GetString("github.oidc-issuer"),
			GitHubOIDCJWKSUri:  viper.GetString("github.oidc-jwks-uri"),
			GitHubOIDCAudience: viper.GetString("github.oidc-audience"),
			GitHubOIDCLegacyAudiences: splitCommaSeparated(
				viper.GetString("github.oidc-legacy-audiences"),
			),
			GitHubOIDCAlgs: []string{"RS256"},
		},
		Keycloak: KeycloakConfiguration{
			BaseURL:           base,
			Realm:             realm,
			AdminClientID:     viper.GetString("kc.admin-client-id"),
			AdminClientSecret: viper.GetString("kc.admin-client-secret"),
			TokenURL:          issuer + "/protocol/openid-connect/token",

			WorkloadRealm:             viper.GetString("kc.workload-realm"),
			WorkloadAdminClientID:     viper.GetString("kc.workload-admin-client-id"),
			WorkloadAdminClientSecret: viper.GetString("kc.workload-admin-client-secret"),
			WorkloadTokenURL: fmt.Sprintf("%s/realms/%s/protocol/openid-connect/token",
				base, viper.GetString("kc.workload-realm")),
			WorkloadAudience: viper.GetString("kc.workload-audience"),
		},
		Gateway: GatewayConfiguration{
			Scheme:        viper.GetString("gateway.scheme"),
			Port:          viper.GetString("gateway.port"),
			ClusterDomain: viper.GetString("gateway.cluster-domain"),
		},
		Database: DatabaseConfiguration{
			URL:                      viper.GetString("database.url"),
			StateQueryDSN:            viper.GetString("database.state-query-dsn"),
			StateQueryTenantPassword: viper.GetString("database.state-query-tenant-password"),
		},
		Stripe: StripeConfiguration{
			SecretKey:          viper.GetString("stripe.secret-key"),
			WebhookSecret:      viper.GetString("stripe.webhook-secret"),
			CheckoutSuccessURL: viper.GetString("stripe.checkout-success-url"),
			CheckoutCancelURL:  viper.GetString("stripe.checkout-cancel-url"),
			PortalReturnURL:    viper.GetString("stripe.portal-return-url"),
		},
		Chat: ChatConfiguration{
			BaseURL: viper.GetString("chat.base-url"),
			APIKey:  viper.GetString("chat.api-key"),
			Model:   viper.GetString("chat.model"),
		},
		SignedServiceURL: SignedServiceURLConfiguration{
			BaseURL:    strings.TrimRight(strings.TrimSpace(viper.GetString("signed-service-url.base-url")), "/"),
			Secret:     viper.GetString("signed-service-url.secret"),
			SecretFile: strings.TrimSpace(viper.GetString("signed-service-url.secret-file")),
		},
		Usage: UsageConfiguration{
			DatabaseURL:       strings.TrimSpace(viper.GetString("usage.database-url")),
			QueryWebhookURL:   strings.TrimSpace(viper.GetString("usage.query-webhook-url")),
			QueryHMACSecret:   strings.TrimSpace(viper.GetString("usage.query-hmac-secret")),
			QueryResultBucket: strings.TrimSpace(viper.GetString("usage.query-result-bucket")),
			QueryResultPrefix: strings.Trim(strings.TrimSpace(viper.GetString("usage.query-result-prefix")), "/"),
			QueryCluster:      strings.TrimSpace(viper.GetString("usage.query-cluster")),
			QueryEnvironment:  strings.TrimSpace(viper.GetString("usage.query-environment")),
			QueryTimeout:      usageQueryTimeout,
			QueryPollInterval: usagePollInterval,
			MaxResponseBytes:  usageMaxResponseBytes,
		},
		Metrics: MetricsConfiguration{Addr: viper.GetString("metrics.addr")},
		ProductAnalytics: ProductAnalyticsConfiguration{
			Enabled:          viper.GetBool("product-analytics.enabled"),
			Host:             strings.TrimSpace(viper.GetString("product-analytics.host")),
			ProjectToken:     strings.TrimSpace(viper.GetString("product-analytics.project-token")),
			IdentityKey:      strings.TrimSpace(viper.GetString("product-analytics.identity-key")),
			Environment:      strings.TrimSpace(viper.GetString("telemetry.environment")),
			ExcludedSubjects: splitCommaSeparated(viper.GetString("product-analytics.excluded-subjects")),
		},
		Telemetry: TelemetryConfiguration{
			Endpoint:         viper.GetString("telemetry.endpoint"),
			Protocol:         viper.GetString("telemetry.protocol"),
			ServiceName:      viper.GetString("telemetry.service-name"),
			ServiceNamespace: viper.GetString("telemetry.service-namespace"),
			Environment:      viper.GetString("telemetry.environment"),
			ResourceAttrs:    viper.GetString("telemetry.resource-attributes"),
		},
	}
	if cfg.Keycloak.AdminClientSecret == "" {
		return nil, fmt.Errorf("KC_ADMIN_CLIENT_SECRET is required")
	}
	if (cfg.Chat.BaseURL == "") != (cfg.Chat.APIKey == "") {
		return nil, fmt.Errorf("chat configuration requires both LITELLM_BASE_URL and LITELLM_API_KEY")
	}
	if err := validateSignedServiceURLConfiguration(cfg.SignedServiceURL); err != nil {
		return nil, err
	}
	if err := validateUsageConfiguration(cfg.Usage); err != nil {
		return nil, err
	}
	return cfg, nil
}

func validateSignedServiceURLConfiguration(cfg SignedServiceURLConfiguration) error {
	if cfg.BaseURL == "" {
		if cfg.Secret != "" {
			return fmt.Errorf("SIGNED_SERVICE_URL_SECRET requires SIGNED_SERVICE_URL_BASE_URL")
		}
		if cfg.SecretFile != "" {
			return fmt.Errorf("SIGNED_SERVICE_URL_SECRET_FILE requires SIGNED_SERVICE_URL_BASE_URL")
		}
		return nil
	}
	parsed, err := url.Parse(cfg.BaseURL)
	if err != nil {
		return newSanitizedError("invalid SIGNED_SERVICE_URL_BASE_URL", err)
	}
	if parsed.Scheme != "https" || parsed.Host == "" || parsed.User != nil || parsed.RawQuery != "" || parsed.Fragment != "" || (parsed.Path != "" && parsed.Path != "/") || (parsed.RawPath != "" && parsed.RawPath != "/") {
		return fmt.Errorf("SIGNED_SERVICE_URL_BASE_URL must be a bare HTTPS origin")
	}
	if cfg.Secret != "" && len([]byte(cfg.Secret)) < sha256.Size {
		return fmt.Errorf("SIGNED_SERVICE_URL_SECRET must be at least %d bytes", sha256.Size)
	}
	return nil
}

func validateUsageConfiguration(cfg UsageConfiguration) error {
	if cfg.QueryTimeout < minUsageQueryTimeout || cfg.QueryTimeout > maxUsageQueryTimeout {
		return fmt.Errorf("USAGE_QUERY_TIMEOUT must be between 1s and 2m")
	}
	if cfg.QueryPollInterval < 250*time.Millisecond || cfg.QueryPollInterval > 5*time.Second {
		return fmt.Errorf("USAGE_QUERY_POLL_INTERVAL must be between 250ms and 5s")
	}
	if cfg.MaxResponseBytes < minUsageResponseSize || cfg.MaxResponseBytes > maxUsageResponseSize {
		return fmt.Errorf("USAGE_MAX_RESPONSE_BYTES must be between 65536 and 33554432")
	}
	if cfg.DatabaseURL == "" {
		return nil
	}
	for name, value := range map[string]string{
		"USAGE_QUERY_HMAC_SECRET":   cfg.QueryHMACSecret,
		"USAGE_QUERY_RESULT_BUCKET": cfg.QueryResultBucket,
		"USAGE_QUERY_RESULT_PREFIX": cfg.QueryResultPrefix,
		"USAGE_QUERY_CLUSTER":       cfg.QueryCluster,
		"USAGE_QUERY_ENVIRONMENT":   cfg.QueryEnvironment,
	} {
		if value == "" {
			return fmt.Errorf("USAGE_DATABASE_URL requires %s", name)
		}
	}
	databaseURL, err := url.Parse(cfg.DatabaseURL)
	if err != nil {
		return newSanitizedError("invalid usage database URL", err)
	}
	if (databaseURL.Scheme != "postgres" && databaseURL.Scheme != "postgresql") || databaseURL.Host == "" {
		return fmt.Errorf("invalid usage database URL")
	}
	postgresConfig, err := pgxpool.ParseConfig(cfg.DatabaseURL)
	if err != nil {
		return newSanitizedError("invalid usage database URL", err)
	}
	if postgresConfig.ConnConfig.User != "cyclops_usage_reader" {
		return fmt.Errorf("usage database URL must use cyclops_usage_reader")
	}
	if cfg.QueryWebhookURL != clusterUsageQueryWebhookURL && cfg.QueryWebhookURL != externalUsageQueryWebhookURL {
		return fmt.Errorf("invalid allocation query webhook URL")
	}
	if cfg.QueryResultBucket != "nanoclaw-telemetry-files" {
		return fmt.Errorf("invalid allocation query result bucket")
	}
	if cfg.QueryResultPrefix != "cyclops/usage-query" {
		return fmt.Errorf("invalid allocation query result prefix")
	}
	for _, value := range []string{cfg.QueryCluster, cfg.QueryEnvironment} {
		if !usageQueryPartitionValue.MatchString(value) {
			return fmt.Errorf("invalid allocation query partition")
		}
	}
	return nil
}

func parseUsageQueryTimeout(raw string) (time.Duration, error) {
	value, err := time.ParseDuration(strings.TrimSpace(raw))
	if err != nil {
		return 0, fmt.Errorf("invalid USAGE_QUERY_TIMEOUT: %w", err)
	}
	return value, nil
}

func parseUsagePollInterval(raw string) (time.Duration, error) {
	value, err := time.ParseDuration(strings.TrimSpace(raw))
	if err != nil {
		return 0, fmt.Errorf("invalid USAGE_QUERY_POLL_INTERVAL: %w", err)
	}
	return value, nil
}

func parseUsageMaxResponseBytes(raw string) (int64, error) {
	value, err := strconv.ParseInt(strings.TrimSpace(raw), 10, 64)
	if err != nil {
		return 0, fmt.Errorf("invalid USAGE_MAX_RESPONSE_BYTES: %w", err)
	}
	return value, nil
}
