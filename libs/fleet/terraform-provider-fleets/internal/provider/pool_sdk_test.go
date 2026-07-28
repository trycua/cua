package provider

import (
	"context"
	"testing"

	"github.com/hashicorp/terraform-plugin-framework/diag"
	"github.com/hashicorp/terraform-plugin-framework/types"
	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/cyclops_sdk"
)

func TestPoolResourceModelToSDKCreatePoolRequest(t *testing.T) {
	model := poolResourceModel{
		Name:               types.StringValue("example"),
		Namespace:          types.StringNull(),
		Replicas:           types.Int64Value(2),
		CPUCores:           types.Int64Value(4),
		Memory:             types.StringValue("8Gi"),
		ContainerDiskImage: types.StringValue("registry.example/image:latest"),
		ImagePullSecret:    types.StringValue("pull-secret"),
		Runtime:            types.StringValue("kubevirt"),
		Firmware:           types.StringValue("bios"),
	}

	var diagnostics diag.Diagnostics
	request := model.toSDKCreatePoolRequest(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}

	var _ cyclops_sdk.CreatePoolRequest = request
	if request.Namespace != "example" {
		t.Fatalf("namespace = %q, want example", request.Namespace)
	}

	pool := model.toSDKPool(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if pool.Metadata.Namespace != "example" {
		t.Fatalf("pool namespace = %q, want example", pool.Metadata.Namespace)
	}
	if request.Spec.Replicas != 2 {
		t.Fatalf("replicas = %d, want 2", request.Spec.Replicas)
	}
}
