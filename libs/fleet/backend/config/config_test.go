package config

import (
	"os"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/spf13/pflag"
	"github.com/spf13/viper"
)

func TestLoadConfig_UsesPublicIssuerForTokenURL(t *testing.T) {
	viper.Reset()
	t.Cleanup(viper.Reset)
	t.Setenv("KC_BASE_URL", "http://10.43.161.164:8080")
	t.Setenv("KC_ISSUER", "https://auth.cua.ai/realms/cyclops-cs")
	t.Setenv("KC_ADMIN_CLIENT_SECRET", "secret")
	RegisterFlags(pflag.NewFlagSet("test", pflag.ContinueOnError))

	cfg, err := LoadConfig()
	if err != nil {
		t.Fatalf("LoadConfig() error = %v", err)
	}

	if got, want := cfg.Keycloak.TokenURL, "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"; got != want {
		t.Fatalf("Keycloak.TokenURL = %q, want %q", got, want)
	}
}

func TestLoadConfig_TelemetryDefaults(t *testing.T) {
	viper.Reset()
	t.Cleanup(viper.Reset)
	t.Setenv("KC_ADMIN_CLIENT_SECRET", "secret")
	RegisterFlags(pflag.NewFlagSet("test", pflag.ContinueOnError))

	cfg, err := LoadConfig()
	if err != nil {
		t.Fatalf("LoadConfig() error = %v", err)
	}

	if got, want := cfg.Telemetry.Endpoint, "https://otel.cua.ai"; got != want {
		t.Fatalf("Telemetry.Endpoint = %q, want %q", got, want)
	}
	if got, want := cfg.Telemetry.Protocol, "http/protobuf"; got != want {
		t.Fatalf("Telemetry.Protocol = %q, want %q", got, want)
	}
	if got, want := cfg.Telemetry.ServiceName, "cyclops-cs-backend"; got != want {
		t.Fatalf("Telemetry.ServiceName = %q, want %q", got, want)
	}
	if got, want := cfg.Telemetry.ServiceNamespace, "cyclops-cs"; got != want {
		t.Fatalf("Telemetry.ServiceNamespace = %q, want %q", got, want)
	}
	if got, want := cfg.Telemetry.Environment, "production"; got != want {
		t.Fatalf("Telemetry.Environment = %q, want %q", got, want)
	}
}

func TestLoadConfig_StripeBillingValues(t *testing.T) {
	t.Setenv("KC_ADMIN_CLIENT_SECRET", "secret")
	t.Setenv("STRIPE_SECRET_KEY", "sk_test_server_only")
	t.Setenv("STRIPE_WEBHOOK_SECRET", "whsec_test")
	t.Setenv("STRIPE_CHECKOUT_SUCCESS_URL", "https://run.example.test/billing?checkout=success")
	t.Setenv("STRIPE_CHECKOUT_CANCEL_URL", "https://run.example.test/billing?checkout=cancelled")
	t.Setenv("STRIPE_PORTAL_RETURN_URL", "https://run.example.test/billing")
	RegisterFlags(pflag.NewFlagSet("stripe-test", pflag.ContinueOnError))

	cfg, err := LoadConfig()
	if err != nil {
		t.Fatalf("LoadConfig() error = %v", err)
	}

	if got, want := cfg.Stripe.SecretKey, "sk_test_server_only"; got != want {
		t.Fatalf("Stripe.SecretKey = %q, want %q", got, want)
	}
	if got, want := cfg.Stripe.WebhookSecret, "whsec_test"; got != want {
		t.Fatalf("Stripe.WebhookSecret = %q, want %q", got, want)
	}
	if got, want := cfg.Stripe.CheckoutSuccessURL, "https://run.example.test/billing?checkout=success"; got != want {
		t.Fatalf("Stripe.CheckoutSuccessURL = %q, want %q", got, want)
	}
	if got, want := cfg.Stripe.CheckoutCancelURL, "https://run.example.test/billing?checkout=cancelled"; got != want {
		t.Fatalf("Stripe.CheckoutCancelURL = %q, want %q", got, want)
	}
	if got, want := cfg.Stripe.PortalReturnURL, "https://run.example.test/billing"; got != want {
		t.Fatalf("Stripe.PortalReturnURL = %q, want %q", got, want)
	}
}

func TestLoadConfig_StripeBillingDefaultsAbsent(t *testing.T) {
	t.Setenv("KC_ADMIN_CLIENT_SECRET", "secret")
	for _, key := range []string{
		"STRIPE_SECRET_KEY",
		"STRIPE_WEBHOOK_SECRET",
		"STRIPE_CHECKOUT_SUCCESS_URL",
		"STRIPE_CHECKOUT_CANCEL_URL",
		"STRIPE_PORTAL_RETURN_URL",
	} {
		t.Setenv(key, "")
	}
	RegisterFlags(pflag.NewFlagSet("stripe-absent-test", pflag.ContinueOnError))

	cfg, err := LoadConfig()
	if err != nil {
		t.Fatalf("LoadConfig() error = %v", err)
	}
	if cfg.Stripe != (StripeConfiguration{}) {
		t.Fatalf("Stripe = %#v, want zero configuration", cfg.Stripe)
	}
}

func unsetEnv(t *testing.T, key string) {
	t.Helper()
	value, wasSet := os.LookupEnv(key)
	if err := os.Unsetenv(key); err != nil {
		t.Fatalf("Unsetenv(%q): %v", key, err)
	}
	t.Cleanup(func() {
		if wasSet {
			_ = os.Setenv(key, value)
			return
		}
		_ = os.Unsetenv(key)
	})

}

func loadGitHubOIDCTestConfig(t *testing.T) *Configuration {
	t.Helper()
	viper.Reset()
	t.Cleanup(viper.Reset)
	flags := pflag.NewFlagSet("test", pflag.ContinueOnError)
	RegisterFlags(flags)
	cfg, err := LoadConfig()
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	return cfg
}

func TestLoadConfigGitHubOIDCDefaults(t *testing.T) {
	unsetEnv(t, "GITHUB_OIDC_AUDIENCE")
	unsetEnv(t, "GITHUB_OIDC_LEGACY_AUDIENCES")
	t.Setenv("KC_ADMIN_CLIENT_SECRET", "secret")
	cfg := loadGitHubOIDCTestConfig(t)

	if got := cfg.Auth.GitHubOIDCAudience; got != "fleets" {
		t.Fatalf("GitHubOIDCAudience = %q, want fleets", got)
	}
	if got, want := cfg.Auth.GitHubOIDCLegacyAudiences, []string{"cyclops-cs"}; !slices.Equal(got, want) {
		t.Fatalf("GitHubOIDCLegacyAudiences = %#v, want %#v", got, want)
	}
}

func TestLoadConfigGitHubOIDCLegacyAudiencesEnvironmentOverride(t *testing.T) {
	unsetEnv(t, "GITHUB_OIDC_AUDIENCE")
	t.Setenv("KC_ADMIN_CLIENT_SECRET", "secret")
	t.Setenv("GITHUB_OIDC_LEGACY_AUDIENCES", "cyclops-cs, old-fleets")
	cfg := loadGitHubOIDCTestConfig(t)

	if got, want := cfg.Auth.GitHubOIDCLegacyAudiences, []string{"cyclops-cs", "old-fleets"}; !slices.Equal(got, want) {
		t.Fatalf("GitHubOIDCLegacyAudiences = %#v, want %#v", got, want)
	}
}

func TestLoadConfig_StateDatabaseURLs(t *testing.T) {
	viper.Reset()
	t.Cleanup(viper.Reset)
	t.Setenv("KC_ADMIN_CLIENT_SECRET", "secret")
	t.Setenv("DATABASE_URL", "postgres://admin@db/cyclops")
	t.Setenv("STATE_QUERY_DATABASE_DSN", "postgres://db/cyclops?sslmode=require")
	t.Setenv("STATE_QUERY_TENANT_PASSWORD", "tenant-password")
	RegisterFlags(pflag.NewFlagSet("state-database-test", pflag.ContinueOnError))

	cfg, err := LoadConfig()
	if err != nil {
		t.Fatalf("LoadConfig() error = %v", err)
	}
	if got, want := cfg.Database.StateQueryDSN, "postgres://db/cyclops?sslmode=require"; got != want {
		t.Fatalf("Database.StateQueryDSN = %q, want %q", got, want)
	}
	if got, want := cfg.Database.StateQueryTenantPassword, "tenant-password"; got != want {
		t.Fatalf("Database.StateQueryTenantPassword = %q, want %q", got, want)
	}
}

func loadChatTestConfig(t *testing.T) (*Configuration, error) {
	t.Helper()
	viper.Reset()
	t.Cleanup(viper.Reset)
	t.Setenv("KC_ADMIN_CLIENT_SECRET", "secret")
	RegisterFlags(pflag.NewFlagSet("chat-test", pflag.ContinueOnError))
	return LoadConfig()
}

func TestLoadConfig_ChatConfiguration(t *testing.T) {
	t.Setenv("LITELLM_BASE_URL", "https://litellm.example/v1")
	t.Setenv("LITELLM_API_KEY", "secret")
	t.Setenv("LITELLM_MODEL", "browser-bash")
	cfg, err := loadChatTestConfig(t)
	if err != nil {
		t.Fatalf("LoadConfig() error = %v", err)
	}
	if cfg.Chat.BaseURL != "https://litellm.example/v1" || cfg.Chat.APIKey != "secret" || cfg.Chat.Model != "browser-bash" {
		t.Fatalf("Chat = %#v, want configured LiteLLM client", cfg.Chat)
	}
}

func TestLoadConfig_ChatCredentialsMustBePaired(t *testing.T) {
	for name, env := range map[string]map[string]string{
		"missing base URL": {"LITELLM_API_KEY": "secret"},
		"missing API key":  {"LITELLM_BASE_URL": "https://litellm.example/v1"},
	} {
		t.Run(name, func(t *testing.T) {
			for key, value := range env {
				t.Setenv(key, value)
			}
			_, err := loadChatTestConfig(t)
			if err == nil || !strings.Contains(err.Error(), "chat configuration") {
				t.Fatalf("LoadConfig() error = %v, want chat credential error", err)
			}
		})
	}
}

func TestLoadConfig_ChatCredentialsAreOptional(t *testing.T) {
	if _, err := loadChatTestConfig(t); err != nil {
		t.Fatalf("LoadConfig() error = %v, want optional chat credentials", err)
	}
}

func TestLoadConfig_UsageConfiguration(t *testing.T) {
	validDatabaseURL := "postgres://cyclops_usage_reader:secret@db.example/cyclops?sslmode=require"
	defaults := UsageConfiguration{
		QueryWebhookURL:   "http://cua-temporal-webhook.temporal.svc.cluster.local/hooks/opencost-query",
		QueryResultBucket: "nanoclaw-telemetry-files",
		QueryResultPrefix: "cyclops/usage-query",
		QueryCluster:      "kopf-k3s",
		QueryEnvironment:  "production",
		QueryTimeout:      45 * time.Second,
		QueryPollInterval: time.Second,
		MaxResponseBytes:  8388608,
	}
	tests := []struct {
		name      string
		env       map[string]string
		wantError string
		want      UsageConfiguration
	}{
		{name: "absent disables usage", want: defaults},
		{name: "query settings only disable usage", env: map[string]string{"USAGE_QUERY_HMAC_SECRET": "secret"}, want: func() UsageConfiguration { value := defaults; value.QueryHMACSecret = "secret"; return value }()},
		{name: "database requires HMAC secret", env: map[string]string{"USAGE_DATABASE_URL": validDatabaseURL}, wantError: "USAGE_DATABASE_URL requires USAGE_QUERY_HMAC_SECRET"},
		{
			name: "valid values",
			env: map[string]string{
				"USAGE_DATABASE_URL": validDatabaseURL, "USAGE_QUERY_HMAC_SECRET": "secret",
				"USAGE_QUERY_TIMEOUT": "60s", "USAGE_QUERY_POLL_INTERVAL": "500ms", "USAGE_MAX_RESPONSE_BYTES": "1048576",
			},
			want: func() UsageConfiguration {
				value := defaults
				value.DatabaseURL = validDatabaseURL
				value.QueryHMACSecret = "secret"
				value.QueryTimeout = time.Minute
				value.QueryPollInterval = 500 * time.Millisecond
				value.MaxResponseBytes = 1048576
				return value
			}(),
		},
		{name: "libpq database DSN is rejected", env: map[string]string{"USAGE_DATABASE_URL": "user=cyclops_usage_reader host=db.example dbname=cyclops", "USAGE_QUERY_HMAC_SECRET": "secret"}, wantError: "invalid usage database URL"},
		{name: "database reader role is required", env: map[string]string{"USAGE_DATABASE_URL": "postgres://application@db.example/cyclops", "USAGE_QUERY_HMAC_SECRET": "secret"}, wantError: "usage database URL must use cyclops_usage_reader"},
		{name: "external webhook remains allowed", env: map[string]string{"USAGE_DATABASE_URL": validDatabaseURL, "USAGE_QUERY_HMAC_SECRET": "secret", "USAGE_QUERY_WEBHOOK_URL": externalUsageQueryWebhookURL}, want: func() UsageConfiguration {
			value := defaults
			value.DatabaseURL = validDatabaseURL
			value.QueryHMACSecret = "secret"
			value.QueryWebhookURL = externalUsageQueryWebhookURL
			return value
		}()},
		{name: "webhook is allow-listed", env: map[string]string{"USAGE_DATABASE_URL": validDatabaseURL, "USAGE_QUERY_HMAC_SECRET": "secret", "USAGE_QUERY_WEBHOOK_URL": "https://example.test/hooks/opencost-query"}, wantError: "invalid allocation query webhook URL"},
		{name: "result bucket is allow-listed", env: map[string]string{"USAGE_DATABASE_URL": validDatabaseURL, "USAGE_QUERY_HMAC_SECRET": "secret", "USAGE_QUERY_RESULT_BUCKET": "other"}, wantError: "invalid allocation query result bucket"},
		{name: "malformed timeout is rejected while disabled", env: map[string]string{"USAGE_QUERY_TIMEOUT": "not-a-duration"}, wantError: "invalid USAGE_QUERY_TIMEOUT"},
		{name: "malformed poll interval is rejected while disabled", env: map[string]string{"USAGE_QUERY_POLL_INTERVAL": "not-a-duration"}, wantError: "invalid USAGE_QUERY_POLL_INTERVAL"},
		{name: "malformed response limit is rejected while disabled", env: map[string]string{"USAGE_MAX_RESPONSE_BYTES": "not-a-number"}, wantError: "invalid USAGE_MAX_RESPONSE_BYTES"},
		{name: "short timeout is rejected while disabled", env: map[string]string{"USAGE_QUERY_TIMEOUT": "500ms"}, wantError: "USAGE_QUERY_TIMEOUT must be between 1s and 2m"},
		{name: "short poll interval is rejected while disabled", env: map[string]string{"USAGE_QUERY_POLL_INTERVAL": "10ms"}, wantError: "USAGE_QUERY_POLL_INTERVAL must be between 250ms and 5s"},
		{name: "small response limit is rejected while disabled", env: map[string]string{"USAGE_MAX_RESPONSE_BYTES": "1"}, wantError: "USAGE_MAX_RESPONSE_BYTES must be between 65536 and 33554432"},
	}

	keys := []string{
		"USAGE_DATABASE_URL", "USAGE_QUERY_WEBHOOK_URL", "USAGE_QUERY_HMAC_SECRET",
		"USAGE_QUERY_RESULT_BUCKET", "USAGE_QUERY_RESULT_PREFIX", "USAGE_QUERY_CLUSTER",
		"USAGE_QUERY_ENVIRONMENT", "USAGE_QUERY_TIMEOUT", "USAGE_QUERY_POLL_INTERVAL",
		"USAGE_MAX_RESPONSE_BYTES",
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			viper.Reset()
			t.Cleanup(viper.Reset)
			t.Setenv("KC_ADMIN_CLIENT_SECRET", "secret")
			for _, key := range keys {
				t.Setenv(key, "")
			}
			for key, value := range test.env {
				t.Setenv(key, value)
			}
			RegisterFlags(pflag.NewFlagSet("test", pflag.ContinueOnError))

			cfg, err := LoadConfig()
			if test.wantError != "" {
				if err == nil || !strings.Contains(err.Error(), test.wantError) {
					t.Fatalf("LoadConfig() error = %v, want containing %q", err, test.wantError)
				}
				return
			}
			if err != nil {
				t.Fatalf("LoadConfig() error = %v", err)
			}
			if cfg.Usage != test.want {
				t.Fatalf("Usage = %#v, want %#v", cfg.Usage, test.want)
			}
		})
	}
}

func TestLoadConfig_ProductAnalyticsValues(t *testing.T) {
	viper.Reset()
	t.Cleanup(viper.Reset)
	t.Setenv("KC_ADMIN_CLIENT_SECRET", "secret")
	t.Setenv("FLEET_ANALYTICS_ENABLED", "true")
	t.Setenv("POSTHOG_HOST", "https://eu.i.posthog.com")
	t.Setenv("POSTHOG_PROJECT_TOKEN", "phc_test")
	t.Setenv("POSTHOG_IDENTITY_KEY", "identity-test-key")
	t.Setenv("FLEET_ANALYTICS_EXCLUDED_SUBS", "internal-1, internal-2")
	t.Setenv("OTEL_ENVIRONMENT", "production")
	RegisterFlags(pflag.NewFlagSet("analytics-test", pflag.ContinueOnError))

	cfg, err := LoadConfig()
	if err != nil {
		t.Fatalf("LoadConfig() error = %v", err)
	}
	if !cfg.ProductAnalytics.Enabled || cfg.ProductAnalytics.Host != "https://eu.i.posthog.com" || cfg.ProductAnalytics.ProjectToken != "phc_test" || cfg.ProductAnalytics.IdentityKey != "identity-test-key" {
		t.Fatalf("ProductAnalytics = %#v", cfg.ProductAnalytics)
	}
	if got, want := cfg.ProductAnalytics.Environment, "production"; got != want {
		t.Fatalf("Environment = %q, want %q", got, want)
	}
	if got, want := cfg.ProductAnalytics.ExcludedSubjects, []string{"internal-1", "internal-2"}; !slices.Equal(got, want) {
		t.Fatalf("ExcludedSubjects = %#v, want %#v", got, want)
	}
}
func loadSignedServiceURLTestConfig(t *testing.T, env map[string]string) (*Configuration, error) {
	t.Helper()
	viper.Reset()
	t.Cleanup(viper.Reset)
	t.Setenv("KC_ADMIN_CLIENT_SECRET", "secret")
	for _, key := range []string{"SIGNED_SERVICE_URL_BASE_URL", "SIGNED_SERVICE_URL_SECRET", "SIGNED_SERVICE_URL_SECRET_FILE"} {
		t.Setenv(key, "")
	}
	for key, value := range env {
		t.Setenv(key, value)
	}
	RegisterFlags(pflag.NewFlagSet("signed-service-url-test", pflag.ContinueOnError))
	return LoadConfig()
}

func TestLoadConfig_SignedServiceURLConfiguration(t *testing.T) {
	const secret = "12345678901234567890123456789012"

	t.Run("validates and normalizes configured values", func(t *testing.T) {
		cfg, err := loadSignedServiceURLTestConfig(t, map[string]string{
			"SIGNED_SERVICE_URL_BASE_URL": "https://run.cua.ai/",
			"SIGNED_SERVICE_URL_SECRET":   secret,
		})
		if err != nil {
			t.Fatalf("LoadConfig() error = %v", err)
		}
		if got, want := cfg.SignedServiceURL.BaseURL, "https://run.cua.ai"; got != want {
			t.Fatalf("SignedServiceURL.BaseURL = %q, want %q", got, want)
		}
		if got, want := cfg.SignedServiceURL.Secret, secret; got != want {
			t.Fatalf("SignedServiceURL.Secret = %q, want configured secret", got)
		}
	})

	invalidBaseURLs := map[string]string{
		"HTTP":          "http://run.cua.ai",
		"userinfo":      "https://user@run.cua.ai",
		"query":         "https://run.cua.ai?source=test",
		"fragment":      "https://run.cua.ai#fragment",
		"non-root path": "https://run.cua.ai/services",
	}
	for name, baseURL := range invalidBaseURLs {
		t.Run("rejects "+name, func(t *testing.T) {
			_, err := loadSignedServiceURLTestConfig(t, map[string]string{
				"SIGNED_SERVICE_URL_BASE_URL": baseURL,
				"SIGNED_SERVICE_URL_SECRET":   secret,
			})
			if err == nil || !strings.Contains(err.Error(), "SIGNED_SERVICE_URL_BASE_URL") {
				t.Fatalf("LoadConfig() error = %v, want invalid base URL error", err)
			}
		})
	}

	for name, env := range map[string]map[string]string{
		"short secret":     {"SIGNED_SERVICE_URL_BASE_URL": "https://run.cua.ai", "SIGNED_SERVICE_URL_SECRET": "too-short"},
		"missing base URL": {"SIGNED_SERVICE_URL_SECRET": secret},
	} {
		t.Run("rejects "+name, func(t *testing.T) {
			_, err := loadSignedServiceURLTestConfig(t, env)
			if err == nil || !strings.Contains(err.Error(), "SIGNED_SERVICE_URL") {
				t.Fatalf("LoadConfig() error = %v, want signed URL configuration error", err)
			}
		})
	}

	t.Run("base URL without secret keeps the feature disabled during rollout", func(t *testing.T) {
		cfg, err := loadSignedServiceURLTestConfig(t, map[string]string{
			"SIGNED_SERVICE_URL_BASE_URL": "https://run.cua.ai",
		})
		if err != nil {
			t.Fatalf("LoadConfig() error = %v, want disabled configuration", err)
		}
		if got, want := cfg.SignedServiceURL.BaseURL, "https://run.cua.ai"; got != want {
			t.Fatalf("SignedServiceURL.BaseURL = %q, want %q", got, want)
		}
		if cfg.SignedServiceURL.Secret != "" {
			t.Fatalf("SignedServiceURL.Secret = %q, want empty optional secret", cfg.SignedServiceURL.Secret)
		}
	})

	t.Run("accepts a non-sensitive secret file path", func(t *testing.T) {
		cfg, err := loadSignedServiceURLTestConfig(t, map[string]string{
			"SIGNED_SERVICE_URL_BASE_URL":    "https://run.cua.ai",
			"SIGNED_SERVICE_URL_SECRET_FILE": "/var/run/signed-service-url/hmac_secret",
		})
		if err != nil {
			t.Fatalf("LoadConfig() error = %v", err)
		}
		if got, want := cfg.SignedServiceURL.SecretFile, "/var/run/signed-service-url/hmac_secret"; got != want {
			t.Fatalf("SignedServiceURL.SecretFile = %q, want %q", got, want)
		}
	})

	t.Run("both values absent disables feature", func(t *testing.T) {
		cfg, err := loadSignedServiceURLTestConfig(t, nil)
		if err != nil {
			t.Fatalf("LoadConfig() error = %v, want disabled configuration", err)
		}
		if cfg.SignedServiceURL != (SignedServiceURLConfiguration{}) {
			t.Fatalf("SignedServiceURL = %#v, want zero configuration", cfg.SignedServiceURL)
		}
	})
}
