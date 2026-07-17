package config

import (
	"testing"

	"github.com/spf13/pflag"
)

func TestLoadConfig_TelemetryDefaults(t *testing.T) {
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
