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
	"fmt"
	"strings"

	"github.com/spf13/pflag"
	"github.com/spf13/viper"
)

type Configuration struct {
	WebServer WebServerConfiguration
	Auth      AuthConfiguration
	Keycloak  KeycloakConfiguration
	Gateway   GatewayConfiguration
	Metrics   MetricsConfiguration
	Telemetry TelemetryConfiguration
}

type WebServerConfiguration struct {
	Addr string
}

// AuthConfiguration follows the grt naming so the JWKS / verifier code
// reads like the upstream template.
type AuthConfiguration struct {
	Issuer           string   // https://auth.cua.ai/realms/cyclops-cs
	JWKSUri          string   // <Issuer>/protocol/openid-connect/certs
	SigningAlgs      []string // RS256, RS512, ES256
	SPAClientID      string
	KeyClientPfx     string // pool/gateway key client-id prefix ("key-")
	UserKeyClientPfx string // per-user key client-id prefix ("ukey-")
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


type MetricsConfiguration struct {
	Addr string // METRICS_ADDR — Prometheus listen addr
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
	{"kc.admin-client-id", "kc-admin-client-id", "KC_ADMIN_CLIENT_ID", "cyclops-cs-backend", "Keycloak admin client id"},
	{"kc.admin-client-secret", "kc-admin-client-secret", "KC_ADMIN_CLIENT_SECRET", "", "Keycloak admin client secret (required)"},
	{"kc.workload-realm", "kc-workload-realm", "KC_WORKLOAD_REALM", "workloads", "Keycloak realm AWS/OIDC trusts for pool VM tokens"},
	{"kc.workload-admin-client-id", "kc-workload-admin-client-id", "KC_WORKLOAD_ADMIN_CLIENT_ID", "workloads-admin", "admin client id IN the workloads realm"},
	{"kc.workload-admin-client-secret", "kc-workload-admin-client-secret", "KC_WORKLOAD_ADMIN_CLIENT_SECRET", "", "admin client secret for the workloads realm (enables per-tenant pool VM OIDC when set)"},
	{"kc.workload-audience", "kc-workload-audience", "KC_WORKLOAD_AUDIENCE", "sts.amazonaws.com", "aud claim stamped on per-tenant pool VM workload tokens"},
	{"gateway.scheme", "orch-scheme", "ORCH_SCHEME", "http", "orchestrator scheme"},
	{"gateway.port", "orch-port", "ORCH_PORT", "80", "orchestrator port"},
	{"gateway.cluster-domain", "cluster-domain", "CLUSTER_DOMAIN", "svc.cluster.local", "in-cluster DNS domain"},
	{"metrics.addr", "metrics-addr", "METRICS_ADDR", ":9091", "Prometheus metrics listen address"},
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

func LoadConfig() (*Configuration, error) {
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
			Issuer:           issuer,
			JWKSUri:          realmPath + "/protocol/openid-connect/certs",
			SigningAlgs:      []string{"RS256", "RS512", "ES256"},
			SPAClientID:      viper.GetString("kc.spa-client-id"),
			KeyClientPfx:     viper.GetString("kc.key-client-prefix"),
			UserKeyClientPfx: viper.GetString("kc.user-key-client-prefix"),
		},
		Keycloak: KeycloakConfiguration{
			BaseURL:           base,
			Realm:             realm,
			AdminClientID:     viper.GetString("kc.admin-client-id"),
			AdminClientSecret: viper.GetString("kc.admin-client-secret"),
			TokenURL:          realmPath + "/protocol/openid-connect/token",

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
		Metrics:   MetricsConfiguration{Addr: viper.GetString("metrics.addr")},
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
	return cfg, nil
}
