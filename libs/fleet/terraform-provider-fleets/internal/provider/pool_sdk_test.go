package provider

import (
	"context"
	"testing"

	"github.com/hashicorp/terraform-plugin-framework/attr"
	"github.com/hashicorp/terraform-plugin-framework/diag"
	"github.com/hashicorp/terraform-plugin-framework/types"
	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/cyclops_sdk_schema"
	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/fleet_sdk"
)

func examplePoolModel() poolResourceModel {
	return poolResourceModel{
		Name:               types.StringValue("example"),
		Namespace:          types.StringNull(),
		TemplateName:       types.StringNull(),
		Replicas:           types.Int64Value(2),
		CPUCores:           types.Int64Value(4),
		Memory:             types.StringValue("8Gi"),
		ContainerDiskImage: types.StringValue("registry.example/image:latest"),
		ImagePullSecret:    types.StringValue("pull-secret"),
		Runtime:            types.StringValue("kubevirt"),
		Firmware:           types.StringValue("bios"),
		Services:           types.SetNull(types.ObjectType{AttrTypes: serviceObjectType()}),
		Autoscaling:        types.ObjectNull(autoscalingObjectType()),
	}
}

func testAutoscalingValue(initial int64) types.Object {
	return types.ObjectValueMust(autoscalingObjectType(), map[string]attr.Value{
		"min_pool_size":     types.Int64Value(0),
		"initial_pool_size": types.Int64Value(initial),
		"max_pool_size":     types.Int64Value(5),
	})
}

func TestPoolResourceModelToSDKCreatePoolRequest(t *testing.T) {
	model := examplePoolModel()

	var diagnostics diag.Diagnostics
	request := model.toSDKCreatePoolRequest(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}

	var _ fleet_sdk.CreatePoolRequest = request
	if request.Namespace != "example" {
		t.Fatalf("namespace = %q, want example", request.Namespace)
	}
	if request.Spec.Replicas != 2 {
		t.Fatalf("replicas = %d, want 2", request.Spec.Replicas)
	}
	if request.Spec.SandboxTemplateRef.Name != "example-template" {
		t.Fatalf("sandboxTemplateRef = %q, want example-template", request.Spec.SandboxTemplateRef.Name)
	}

	pool := model.toSDKPool(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if pool.ApiVersion != "osgym.cua.ai/v1alpha1" || pool.Kind != "OSGymSandboxWarmPool" {
		t.Fatalf("pool identity = %s %s", pool.ApiVersion, pool.Kind)
	}
	if pool.Metadata.Namespace != "example" || pool.Metadata.Name != "example" {
		t.Fatalf("pool metadata = %+v, want namespace and name example", pool.Metadata)
	}
}

func TestPoolResourceModelToSDKAutoscalingUsesInitialPoolSize(t *testing.T) {
	model := examplePoolModel()
	model.Replicas = types.Int64Null()
	model.Autoscaling = testAutoscalingValue(3)

	var diagnostics diag.Diagnostics
	request := model.toSDKCreatePoolRequest(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if request.Spec.Replicas != 3 {
		t.Fatalf("replicas = %d, want initial_pool_size 3", request.Spec.Replicas)
	}
	if request.Spec.Autoscaling == nil {
		t.Fatal("autoscaling was not serialized")
	}
	if request.Spec.Autoscaling.MaxPoolSize == nil || *request.Spec.Autoscaling.MaxPoolSize != 5 {
		t.Fatalf("max_pool_size = %v, want 5", request.Spec.Autoscaling.MaxPoolSize)
	}
}

func TestPoolResourceModelToSDKAutoscalingNullInitialPoolSizeUsesZero(t *testing.T) {
	model := examplePoolModel()
	model.Replicas = types.Int64Null()
	model.Autoscaling = types.ObjectValueMust(autoscalingObjectType(), map[string]attr.Value{
		"min_pool_size":     types.Int64Value(0),
		"initial_pool_size": types.Int64Null(),
		"max_pool_size":     types.Int64Value(5),
	})

	var diagnostics diag.Diagnostics
	request := model.toSDKCreatePoolRequest(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if request.Spec.Replicas != 0 {
		t.Fatalf("replicas = %d, want zero for null initial_pool_size", request.Spec.Replicas)
	}
}

func TestPoolResourceModelToSDKAutoscalingUnknownInitialPoolSizeUsesZero(t *testing.T) {
	model := examplePoolModel()
	model.Replicas = types.Int64Null()
	model.Autoscaling = types.ObjectValueMust(autoscalingObjectType(), map[string]attr.Value{
		"min_pool_size":     types.Int64Value(0),
		"initial_pool_size": types.Int64Unknown(),
		"max_pool_size":     types.Int64Value(5),
	})

	var diagnostics diag.Diagnostics
	request := model.toSDKCreatePoolRequest(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if request.Spec.Replicas != 0 {
		t.Fatalf("replicas = %d, want zero for unknown initial_pool_size", request.Spec.Replicas)
	}
}

func TestPoolResourceModelToSDKAutoscalingNullMaxPoolSizeUsesDefault(t *testing.T) {
	model := examplePoolModel()
	model.Replicas = types.Int64Null()
	model.Autoscaling = types.ObjectValueMust(autoscalingObjectType(), map[string]attr.Value{
		"min_pool_size":     types.Int64Value(0),
		"initial_pool_size": types.Int64Value(3),
		"max_pool_size":     types.Int64Null(),
	})

	var diagnostics diag.Diagnostics
	request := model.toSDKCreatePoolRequest(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if request.Spec.Autoscaling == nil || request.Spec.Autoscaling.MaxPoolSize == nil {
		t.Fatal("autoscaling max_pool_size was not serialized")
	}
	if *request.Spec.Autoscaling.MaxPoolSize != 50 {
		t.Fatalf("max_pool_size = %d, want default 50", *request.Spec.Autoscaling.MaxPoolSize)
	}
}

func TestPoolResourceModelToSDKAutoscalingUnknownMaxPoolSizeUsesDefault(t *testing.T) {
	model := examplePoolModel()
	model.Replicas = types.Int64Null()
	model.Autoscaling = types.ObjectValueMust(autoscalingObjectType(), map[string]attr.Value{
		"min_pool_size":     types.Int64Value(0),
		"initial_pool_size": types.Int64Value(3),
		"max_pool_size":     types.Int64Unknown(),
	})

	var diagnostics diag.Diagnostics
	request := model.toSDKCreatePoolRequest(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if request.Spec.Autoscaling == nil || request.Spec.Autoscaling.MaxPoolSize == nil {
		t.Fatal("autoscaling max_pool_size was not serialized")
	}
	if *request.Spec.Autoscaling.MaxPoolSize != 50 {
		t.Fatalf("max_pool_size = %d, want default 50", *request.Spec.Autoscaling.MaxPoolSize)
	}
}

func TestPoolResourceModelToSDKCreateTemplateRequest(t *testing.T) {
	model := examplePoolModel()

	var diagnostics diag.Diagnostics
	request := model.toSDKCreateTemplateRequest(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}

	var _ fleet_sdk.CreateTemplateRequest = request
	if request.Namespace != "example" {
		t.Fatalf("namespace = %q, want example", request.Namespace)
	}
	if request.Name != "example-template" {
		t.Fatalf("name = %q, want example-template", request.Name)
	}
	vmTemplate := request.Spec.VmTemplate
	if vmTemplate.ContainerDiskImage != "registry.example/image:latest" {
		t.Fatalf("containerDiskImage = %q", vmTemplate.ContainerDiskImage)
	}
	if vmTemplate.CpuCores == nil || *vmTemplate.CpuCores != 4 {
		t.Fatalf("cpuCores = %v, want 4", vmTemplate.CpuCores)
	}
	if vmTemplate.Memory == nil || *vmTemplate.Memory != "8Gi" {
		t.Fatalf("memory = %v, want 8Gi", vmTemplate.Memory)
	}
	if vmTemplate.ImagePullSecret == nil || *vmTemplate.ImagePullSecret != "pull-secret" {
		t.Fatalf("imagePullSecret = %v, want pull-secret", vmTemplate.ImagePullSecret)
	}
	if vmTemplate.Services == nil || len(*vmTemplate.Services) != 0 {
		t.Fatalf("services = %v, want an empty slice", vmTemplate.Services)
	}
}

func TestPoolResourceModelFromSDKObjects(t *testing.T) {
	replicas := uint32(2)
	readyReplicas := uint32(1)
	cpuCores := uint32(4)
	memory := "8Gi"
	services := []cyclops_sdk_schema.SandboxService{{Name: "ssh", TargetPort: 22}}
	pool := fleet_sdk.Pool{
		Metadata: fleet_sdk.ResourceMetadata{Namespace: "example", Name: "example"},
		Spec: cyclops_sdk_schema.OsGymSandboxWarmPoolSpec{
			Replicas:           2,
			SandboxTemplateRef: cyclops_sdk_schema.SandboxTemplateRef{Name: "example-template"},
		},
		Status: &cyclops_sdk_schema.OsGymSandboxWarmPoolStatus{Replicas: &replicas, ReadyReplicas: &readyReplicas},
	}
	template := fleet_sdk.Template{
		Metadata: fleet_sdk.ResourceMetadata{Namespace: "example", Name: "example-template"},
		Spec: cyclops_sdk_schema.OsGymSandboxTemplateSpec{VmTemplate: cyclops_sdk_schema.VmTemplate{
			ContainerDiskImage: "registry.example/image:latest", CpuCores: &cpuCores, Memory: &memory, Services: &services,
		}},
	}

	var model poolResourceModel
	var diagnostics diag.Diagnostics
	model.fromSDKPool(pool, &diagnostics)
	model.fromSDKTemplate(context.Background(), &template, &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}

	if model.ID.ValueString() != "example" || model.Namespace.ValueString() != "example" {
		t.Fatalf("id = %q, namespace = %q", model.ID.ValueString(), model.Namespace.ValueString())
	}
	if model.TemplateName.ValueString() != "example-template" {
		t.Fatalf("template_name = %q", model.TemplateName.ValueString())
	}
	if model.Replicas.ValueInt64() != 2 {
		t.Fatalf("replicas = %d, want 2", model.Replicas.ValueInt64())
	}
	if model.CurrentReplicas.ValueInt64() != 2 || model.ReadyReplicas.ValueInt64() != 1 {
		t.Fatalf("status replicas = %d, ready = %d", model.CurrentReplicas.ValueInt64(), model.ReadyReplicas.ValueInt64())
	}
	if model.Memory.ValueString() != "8Gi" || model.CPUCores.ValueInt64() != 4 {
		t.Fatalf("memory = %q, cpu_cores = %d", model.Memory.ValueString(), model.CPUCores.ValueInt64())
	}
	if length := len(model.Services.Elements()); length != 1 {
		t.Fatalf("service count = %d, want 1", length)
	}
}

func TestPoolResourceModelFromSDKAutoscalingPreservesLiveReplicas(t *testing.T) {
	minPoolSize := uint32(0)
	initialPoolSize := uint32(2)
	maxPoolSize := uint32(5)
	pool := fleet_sdk.Pool{
		Metadata: fleet_sdk.ResourceMetadata{Namespace: "example", Name: "example"},
		Spec: cyclops_sdk_schema.OsGymSandboxWarmPoolSpec{
			Replicas:           4,
			SandboxTemplateRef: cyclops_sdk_schema.SandboxTemplateRef{Name: "example-template"},
			Autoscaling: &cyclops_sdk_schema.WarmPoolAutoscaling{
				MinPoolSize: &minPoolSize, InitialPoolSize: &initialPoolSize, MaxPoolSize: &maxPoolSize,
			},
		},
	}

	var model poolResourceModel
	var diagnostics diag.Diagnostics
	model.fromSDKPool(pool, &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if model.Replicas.ValueInt64() != 4 {
		t.Fatalf("replicas = %d, want live autoscaled target 4", model.Replicas.ValueInt64())
	}
	if model.Autoscaling.IsNull() {
		t.Fatal("autoscaling was not preserved")
	}
	var autoscaling autoscalingModel
	if !objectValue(context.Background(), model.Autoscaling, &autoscaling, &diagnostics) {
		t.Fatal("autoscaling could not be decoded")
	}
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if autoscaling.MinPoolSize.ValueInt64() != 0 || autoscaling.InitialPoolSize.ValueInt64() != 2 || autoscaling.MaxPoolSize.ValueInt64() != 5 {
		t.Fatalf("autoscaling = min %d, initial %d, max %d; want min 0, initial 2, max 5", autoscaling.MinPoolSize.ValueInt64(), autoscaling.InitialPoolSize.ValueInt64(), autoscaling.MaxPoolSize.ValueInt64())
	}
}

func TestPoolResourceModelFromSDKTemplateClearsMissingTemplate(t *testing.T) {
	model := examplePoolModel()

	var diagnostics diag.Diagnostics
	model.fromSDKTemplate(context.Background(), nil, &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if model.ContainerDiskImage.ValueString() != "" || model.CPUCores.ValueInt64() != 0 {
		t.Fatalf("template attributes were not cleared: %+v", model)
	}
	if model.Services.IsNull() || len(model.Services.Elements()) != 0 {
		t.Fatalf("services = %v, want an empty set", model.Services)
	}
}

func TestPoolResourceModelUpdateRoutesByOwningObject(t *testing.T) {
	state := examplePoolModel()

	replicaChange := state
	replicaChange.Replicas = types.Int64Value(3)
	if replicaChange.warmPoolAttributesEqual(state) {
		t.Fatal("replicas belongs to the warm pool but did not register as changed")
	}
	if !replicaChange.templateAttributesEqual(state) {
		t.Fatal("replicas must not mark the template as changed")
	}

	memoryChange := state
	memoryChange.Memory = types.StringValue("16Gi")
	if memoryChange.templateAttributesEqual(state) {
		t.Fatal("memory belongs to the template but did not register as changed")
	}
	if !memoryChange.warmPoolAttributesEqual(state) {
		t.Fatal("memory must not mark the warm pool as changed")
	}
}
