package provider

import (
	"context"
	"testing"

	frameworkprovider "github.com/hashicorp/terraform-plugin-framework/provider"
	"github.com/hashicorp/terraform-plugin-framework/resource"
)

func TestMetadataUsesFleetsTypeName(t *testing.T) {
	instance := New("1.0.0")()
	response := &frameworkprovider.MetadataResponse{}
	instance.Metadata(context.Background(), frameworkprovider.MetadataRequest{}, response)

	if response.TypeName != "fleets" {
		t.Fatalf("TypeName = %q, want fleets", response.TypeName)
	}
	if response.Version != "1.0.0" {
		t.Fatalf("Version = %q, want 1.0.0", response.Version)
	}
}

func TestPoolResourceMetadataIncludesMigrationAlias(t *testing.T) {
	tests := []struct {
		name     string
		resource resource.Resource
		want     string
	}{
		{name: "current", resource: NewPoolResource(), want: "fleets_pool"},
		{name: "legacy", resource: NewLegacyPoolResource(), want: "cyclops_pool"},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			response := &resource.MetadataResponse{}
			test.resource.Metadata(context.Background(), resource.MetadataRequest{ProviderTypeName: "fleets"}, response)
			if response.TypeName != test.want {
				t.Fatalf("TypeName = %q, want %q", response.TypeName, test.want)
			}
		})
	}
}
