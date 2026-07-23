package cyclops_sdk

import "testing"

func TestGeneratedSdkCompiles(t *testing.T) {
	var _ func(CyclopsConfiguration, HttpClient) (*CyclopsClient, error) = CyclopsClientConnect
}
