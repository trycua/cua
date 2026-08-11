package config

import (
	"os"
	"slices"
	"testing"

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
