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

// The attributes Sandbox.create / Pool.apply can set, named as
// `cua fleet pool export --terraform` emits them.
func parityPoolModel() poolResourceModel {
	model := examplePoolModel()
	model.Command = types.ListValueMust(types.StringType, []attr.Value{types.StringValue("python"), types.StringValue("-m"), types.StringValue("srv")})
	model.ClaimSecrets = types.BoolValue(true)
	model.TTLSecondsAfterCreated = types.Int64Value(7200)
	model.IdleTTLSeconds = types.Int64Value(86400)
	model.TTLPolicy = types.StringValue("Cascade")
	return model
}

func TestPoolParityAttributesAreOptionalOnly(t *testing.T) {
	// Optional without Computed: an old state that lacks them and a server
	// object that lacks them both read as null, so upgrading plans no diff.
	attributes := poolResourceSchema().Attributes
	for _, name := range []string{"command", "claim_secrets", "ttl_seconds_after_created", "idle_ttl_seconds", "ttl_policy"} {
		attribute, ok := attributes[name]
		if !ok {
			t.Fatalf("%s is missing from the schema", name)
		}
		if !attribute.IsOptional() || attribute.IsComputed() || attribute.IsRequired() {
			t.Fatalf("%s optional = %t, computed = %t, required = %t; want optional-only", name, attribute.IsOptional(), attribute.IsComputed(), attribute.IsRequired())
		}
	}
}

func TestPoolParityAttributesReachTheSDK(t *testing.T) {
	model := parityPoolModel()

	var diagnostics diag.Diagnostics
	spec := model.toSDKPoolSpec(context.Background(), &diagnostics)
	template := model.toSDKTemplateSpec(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if spec.TtlSecondsAfterCreated == nil || *spec.TtlSecondsAfterCreated != 7200 {
		t.Fatalf("ttlSecondsAfterCreated = %v, want 7200", spec.TtlSecondsAfterCreated)
	}
	if spec.IdleTtlSeconds == nil || *spec.IdleTtlSeconds != 86400 {
		t.Fatalf("idleTtlSeconds = %v, want 86400", spec.IdleTtlSeconds)
	}
	if spec.TtlPolicy == nil || *spec.TtlPolicy != cyclops_sdk_schema.WarmPoolTtlPolicyCascade {
		t.Fatalf("ttlPolicy = %v, want Cascade", spec.TtlPolicy)
	}
	vm := template.VmTemplate
	if vm.Command == nil || len(*vm.Command) != 3 || (*vm.Command)[2] != "srv" {
		t.Fatalf("command = %v, want [python -m srv]", vm.Command)
	}
	if vm.ClaimSecrets == nil || !*vm.ClaimSecrets {
		t.Fatalf("claimSecrets = %v, want true", vm.ClaimSecrets)
	}
}

func TestPoolParityAttributesOmittedWhenUnset(t *testing.T) {
	model := examplePoolModel()

	var diagnostics diag.Diagnostics
	spec := model.toSDKPoolSpec(context.Background(), &diagnostics)
	template := model.toSDKTemplateSpec(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if spec.TtlSecondsAfterCreated != nil || spec.IdleTtlSeconds != nil || spec.TtlPolicy != nil {
		t.Fatalf("lifecycle fields = %v %v %v, want all omitted", spec.TtlSecondsAfterCreated, spec.IdleTtlSeconds, spec.TtlPolicy)
	}
	if template.VmTemplate.Command != nil || template.VmTemplate.ClaimSecrets != nil {
		t.Fatalf("command = %v, claimSecrets = %v; want omitted", template.VmTemplate.Command, template.VmTemplate.ClaimSecrets)
	}
}

func TestPoolParityEmptyCommandIsSentAsEmpty(t *testing.T) {
	model := examplePoolModel()
	model.Command = types.ListValueMust(types.StringType, []attr.Value{})

	var diagnostics diag.Diagnostics
	template := model.toSDKTemplateSpec(context.Background(), &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	if template.VmTemplate.Command == nil || len(*template.VmTemplate.Command) != 0 {
		t.Fatalf("command = %v, want an explicit empty list", template.VmTemplate.Command)
	}
}

func TestPoolParityRoundTripsThroughTheSDK(t *testing.T) {
	want := parityPoolModel()
	var diagnostics diag.Diagnostics
	pool := want.toSDKPool(context.Background(), &diagnostics)
	template := fleet_sdk.Template{Spec: want.toSDKTemplateSpec(context.Background(), &diagnostics)}

	var got poolResourceModel
	got.fromSDKPool(pool, &diagnostics)
	got.fromSDKTemplate(context.Background(), &template, &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	for name, pair := range map[string][2]attr.Value{
		"command":                   {got.Command, want.Command},
		"claim_secrets":             {got.ClaimSecrets, want.ClaimSecrets},
		"ttl_seconds_after_created": {got.TTLSecondsAfterCreated, want.TTLSecondsAfterCreated},
		"idle_ttl_seconds":          {got.IdleTTLSeconds, want.IdleTTLSeconds},
		"ttl_policy":                {got.TTLPolicy, want.TTLPolicy},
	} {
		if !pair[0].Equal(pair[1]) {
			t.Errorf("%s = %v, want %v", name, pair[0], pair[1])
		}
	}
}

func TestPoolParityOldShapedObjectsReadAsNull(t *testing.T) {
	cpuCores := uint32(4)
	memory := "8Gi"
	pool := fleet_sdk.Pool{
		Metadata: fleet_sdk.ResourceMetadata{Namespace: "example", Name: "example"},
		Spec: cyclops_sdk_schema.OsGymSandboxWarmPoolSpec{
			Replicas: 1, SandboxTemplateRef: cyclops_sdk_schema.SandboxTemplateRef{Name: "example-template"},
		},
	}
	template := fleet_sdk.Template{Spec: cyclops_sdk_schema.OsGymSandboxTemplateSpec{VmTemplate: cyclops_sdk_schema.VmTemplate{
		ContainerDiskImage: "registry.example/image:latest", CpuCores: &cpuCores, Memory: &memory,
	}}}

	var model poolResourceModel
	var diagnostics diag.Diagnostics
	model.fromSDKPool(pool, &diagnostics)
	model.fromSDKTemplate(context.Background(), &template, &diagnostics)
	if diagnostics.HasError() {
		t.Fatalf("unexpected diagnostics: %v", diagnostics)
	}
	for name, value := range map[string]attr.Value{
		"command":                   model.Command,
		"claim_secrets":             model.ClaimSecrets,
		"ttl_seconds_after_created": model.TTLSecondsAfterCreated,
		"idle_ttl_seconds":          model.IdleTTLSeconds,
		"ttl_policy":                model.TTLPolicy,
	} {
		if !value.IsNull() {
			t.Errorf("%s = %v, want null for an object that never set it", name, value)
		}
	}
}

func TestPoolParityUpdateRoutesByOwningObject(t *testing.T) {
	state := examplePoolModel()
	changed := parityPoolModel()

	poolOnly := state
	poolOnly.TTLSecondsAfterCreated = changed.TTLSecondsAfterCreated
	poolOnly.IdleTTLSeconds = changed.IdleTTLSeconds
	poolOnly.TTLPolicy = changed.TTLPolicy
	if poolOnly.warmPoolAttributesEqual(state) {
		t.Fatal("lifecycle fields belong to the warm pool but did not register as changed")
	}
	if !poolOnly.templateAttributesEqual(state) {
		t.Fatal("lifecycle fields must not mark the template as changed")
	}

	templateOnly := state
	templateOnly.Command = changed.Command
	templateOnly.ClaimSecrets = changed.ClaimSecrets
	if templateOnly.templateAttributesEqual(state) {
		t.Fatal("command/claim_secrets belong to the template but did not register as changed")
	}
	if !templateOnly.warmPoolAttributesEqual(state) {
		t.Fatal("command/claim_secrets must not mark the warm pool as changed")
	}
}
