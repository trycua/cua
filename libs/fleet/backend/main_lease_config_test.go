package main

import "testing"

func TestFeatureFlagLeaseConfigDefaults(t *testing.T) {
	config := featureFlagLeaseConfigFromEnv(func(string) string { return "" })
	if config.apiBaseURL != "http://127.0.0.1:8001" || config.namespace != "cyclops-cs" || config.name != "cyclops-feature-flags-writer" {
		t.Fatalf("defaults = %#v", config)
	}
}
func TestFeatureFlagLeaseConfigUsesEnvironment(t *testing.T) {
	values := map[string]string{"KUBERNETES_PROXY_URL": "http://proxy:9000", "FEATURE_FLAG_LEASE_NAMESPACE": "flags", "FEATURE_FLAG_LEASE_NAME": "writer", "HOSTNAME": "pod-1"}
	config := featureFlagLeaseConfigFromEnv(func(key string) string { return values[key] })
	if config.apiBaseURL != "http://proxy:9000" || config.namespace != "flags" || config.name != "writer" || config.holderIdentity != "pod-1" {
		t.Fatalf("config = %#v", config)
	}
}
