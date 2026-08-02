package provider

//go:generate go run ../../cmd/generate-pool-resource -crd ../../../../clusters/base/osgym/crd.yaml -mapping generate/pool_mapping.json -out pool_generated.go

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"

	"github.com/hashicorp/terraform-plugin-framework-jsontypes/jsontypes"
	"github.com/hashicorp/terraform-plugin-framework/attr"
	"github.com/hashicorp/terraform-plugin-framework/diag"
	"github.com/hashicorp/terraform-plugin-framework/path"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/types"
	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/cyclops_sdk"
	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/cyclops_sdk_schema"
)

type poolResource struct {
	client     *cyclops_sdk.CyclopsClient
	legacyType bool
}

func NewPoolResource() resource.Resource { return &poolResource{} }

func NewLegacyPoolResource() resource.Resource { return &poolResource{legacyType: true} }

func (r *poolResource) Metadata(_ context.Context, req resource.MetadataRequest, resp *resource.MetadataResponse) {
	if r.legacyType {
		resp.TypeName = "cyclops_pool"
		return
	}
	resp.TypeName = req.ProviderTypeName + "_pool"
}
func (r *poolResource) Schema(_ context.Context, _ resource.SchemaRequest, resp *resource.SchemaResponse) {
	resp.Schema = poolResourceSchema()
}
func (r *poolResource) Configure(_ context.Context, req resource.ConfigureRequest, resp *resource.ConfigureResponse) {
	if req.ProviderData == nil {
		return
	}
	apiClient, ok := req.ProviderData.(*cyclops_sdk.CyclopsClient)
	if !ok {
		resp.Diagnostics.AddError("Unexpected provider data", fmt.Sprintf("expected *cyclops_sdk.CyclopsClient, got %T", req.ProviderData))
		return
	}
	r.client = apiClient
}

func (r *poolResource) Create(ctx context.Context, req resource.CreateRequest, resp *resource.CreateResponse) {
	var plan poolResourceModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	createRequest := plan.toSDKCreatePoolRequest(ctx, &resp.Diagnostics)
	if resp.Diagnostics.HasError() {
		return
	}
	created, err := r.client.CreatePool(createRequest)
	if err != nil {
		resp.Diagnostics.AddError("Unable to create Cyclops pool", err.Error())
		return
	}
	created, err = r.client.GetPool(created.Metadata.Name)
	if err != nil {
		resp.Diagnostics.AddError("Pool created but could not be read", err.Error())
		return
	}
	plan.fromSDKPool(ctx, created, &resp.Diagnostics)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *poolResource) Read(ctx context.Context, req resource.ReadRequest, resp *resource.ReadResponse) {
	var state poolResourceModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	pool, err := r.client.GetPool(state.Name.ValueString())
	if isNotFound(err) {
		resp.State.RemoveResource(ctx)
		return
	}
	if err != nil {
		resp.Diagnostics.AddError("Unable to read Cyclops pool", err.Error())
		return
	}
	state.fromSDKPool(ctx, pool, &resp.Diagnostics)
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *poolResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan poolResourceModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	pool := plan.toSDKPool(ctx, &resp.Diagnostics)
	if resp.Diagnostics.HasError() {
		return
	}
	updated, err := r.client.UpdatePool(pool)
	if err != nil {
		resp.Diagnostics.AddError("Unable to update Cyclops pool", err.Error())
		return
	}
	updated, err = r.client.GetPool(updated.Metadata.Name)
	if err != nil {
		resp.Diagnostics.AddError("Pool updated but could not be read", err.Error())
		return
	}
	plan.fromSDKPool(ctx, updated, &resp.Diagnostics)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *poolResource) Delete(ctx context.Context, req resource.DeleteRequest, resp *resource.DeleteResponse) {
	var state poolResourceModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if err := r.client.DeletePool(poolIdentity(state.Namespace.ValueString(), state.Name.ValueString())); err != nil {
		resp.Diagnostics.AddError("Unable to delete Cyclops pool", err.Error())
	}
}

func (r *poolResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("id"), req.ID)...)
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("name"), req.ID)...)
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("namespace"), req.ID)...)
}

func (m poolResourceModel) toSDKCreatePoolRequest(ctx context.Context, diagnostics *diag.Diagnostics) cyclops_sdk.CreatePoolRequest {
	pool := m.toSDKPool(ctx, diagnostics)
	return cyclops_sdk.CreatePoolRequest{Namespace: m.Name.ValueString(), Spec: pool.Spec}
}

func (m poolResourceModel) toSDKPool(ctx context.Context, diagnostics *diag.Diagnostics) cyclops_sdk.Pool {
	services := []cyclops_sdk_schema.SandboxService{}
	if !m.Services.IsNull() && !m.Services.IsUnknown() {
		var values []serviceModel
		diagnostics.Append(m.Services.ElementsAs(ctx, &values, false)...)
		for _, value := range values {
			protocol := cyclops_sdk_schema.ServiceProtocolTcp
			if value.Protocol.ValueString() == "UDP" {
				protocol = cyclops_sdk_schema.ServiceProtocolUdp
			}
			services = append(services, cyclops_sdk_schema.SandboxService{
				Name: value.Name.ValueString(), TargetPort: uint16(value.TargetPort.ValueInt64()), Protocol: &protocol,
			})
		}
	}
	var autoscaling *cyclops_sdk_schema.WarmPoolAutoscaling
	var autoscalingValue autoscalingModel
	if objectValue(ctx, m.Autoscaling, &autoscalingValue, diagnostics) {
		minPoolSize := uint32(autoscalingValue.MinPoolSize.ValueInt64())
		initialPoolSize := uint32(autoscalingValue.InitialPoolSize.ValueInt64())
		maxPoolSize := uint32(autoscalingValue.MaxPoolSize.ValueInt64())
		autoscaling = &cyclops_sdk_schema.WarmPoolAutoscaling{
			MinPoolSize: &minPoolSize, InitialPoolSize: &initialPoolSize, MaxPoolSize: &maxPoolSize,
		}
	}
	var runtime *cyclops_sdk_schema.RuntimeKind
	switch m.Runtime.ValueString() {
	case "macos":
		value := cyclops_sdk_schema.RuntimeKindMacos
		runtime = &value
	case "gvisor":
		value := cyclops_sdk_schema.RuntimeKindGvisor
		runtime = &value
	}
	var firmware *cyclops_sdk_schema.Firmware
	if m.Firmware.ValueString() == "efi" {
		value := cyclops_sdk_schema.FirmwareEfi
		firmware = &value
	}
	imagePullSecret := m.ImagePullSecret.ValueString()
	if imagePullSecret == "" {
		imagePullSecret = "ecr-credentials"
	}
	cpuCores := uint32(m.CPUCores.ValueInt64())
	memory := m.Memory.ValueString()
	probes := m.toSDKProbes(diagnostics)
	return cyclops_sdk.Pool{
		ApiVersion: "cua.ai/v1", Kind: "OSGymWorkspacePool",
		Metadata: cyclops_sdk.ResourceMetadata{Namespace: m.Name.ValueString(), Name: m.Name.ValueString()},
		Spec: cyclops_sdk_schema.PoolSpec{
			Replicas: uint32(m.Replicas.ValueInt64()), Autoscaling: autoscaling, Services: &services,
			Template: cyclops_sdk_schema.PoolTemplate{
				Runtime: runtime, ContainerDiskImage: m.ContainerDiskImage.ValueString(), ImagePullSecret: &imagePullSecret,
				CpuCores: &cpuCores, Memory: &memory, Firmware: firmware, Probes: probes,
			},
		},
	}
}

func (m poolResourceModel) toSDKProbes(diagnostics *diag.Diagnostics) **cyclops_sdk_schema.PreservedJson {
	probes := map[string]json.RawMessage{}
	decodeProbe("readinessProbe", m.ReadinessProbeJSON, probes, diagnostics)
	decodeProbe("livenessProbe", m.LivenessProbeJSON, probes, diagnostics)
	if len(probes) == 0 {
		return nil
	}
	encoded, err := json.Marshal(probes)
	if err != nil {
		diagnostics.AddError("Unable to encode probe JSON", err.Error())
		return nil
	}
	value, err := cyclops_sdk_schema.PreservedJsonFromJson(string(encoded))
	if err != nil {
		diagnostics.AddError("Unable to encode probe JSON", err.Error())
		return nil
	}
	return &value
}

func (m *poolResourceModel) fromSDKPool(ctx context.Context, pool cyclops_sdk.Pool, diagnostics *diag.Diagnostics) {
	m.ID = types.StringValue(pool.Metadata.Name)
	m.Name = types.StringValue(pool.Metadata.Name)
	m.Namespace = types.StringValue(pool.Metadata.Namespace)
	m.Replicas = types.Int64Value(int64(pool.Spec.Replicas))
	m.CPUCores = types.Int64Value(optionalUint32(pool.Spec.Template.CpuCores))
	m.Memory = types.StringValue(optionalString(pool.Spec.Template.Memory))
	m.ContainerDiskImage = types.StringValue(pool.Spec.Template.ContainerDiskImage)
	m.ImagePullSecret = types.StringValue(defaultString(pool.Spec.Template.ImagePullSecret, "ecr-credentials"))
	m.Runtime = types.StringValue(runtimeString(pool.Spec.Template.Runtime))
	m.Firmware = types.StringValue(firmwareString(pool.Spec.Template.Firmware))
	probes := sdkProbes(pool.Spec.Template.Probes, diagnostics)
	m.ReadinessProbeJSON = encodeProbe(probes["readinessProbe"], diagnostics)
	m.LivenessProbeJSON = encodeProbe(probes["livenessProbe"], diagnostics)
	serviceType := serviceObjectType()
	serviceValues := make([]attr.Value, 0, len(optionalServices(pool.Spec.Services)))
	for _, value := range optionalServices(pool.Spec.Services) {
		object, diags := types.ObjectValue(serviceType, map[string]attr.Value{
			"name": types.StringValue(value.Name), "target_port": types.Int64Value(int64(value.TargetPort)), "protocol": types.StringValue(serviceProtocolString(value.Protocol)),
		})
		diagnostics.Append(diags...)
		serviceValues = append(serviceValues, object)
	}
	services, diags := types.SetValue(types.ObjectType{AttrTypes: serviceType}, serviceValues)
	diagnostics.Append(diags...)
	m.Services = services
	if pool.Spec.Autoscaling == nil {
		m.Autoscaling = types.ObjectNull(autoscalingObjectType())
	} else {
		object, diags := types.ObjectValue(autoscalingObjectType(), map[string]attr.Value{
			"min_pool_size":     types.Int64Value(optionalUint32(pool.Spec.Autoscaling.MinPoolSize)),
			"initial_pool_size": types.Int64Value(optionalUint32(pool.Spec.Autoscaling.InitialPoolSize)),
			"max_pool_size":     types.Int64Value(optionalUint32(pool.Spec.Autoscaling.MaxPoolSize)),
		})
		diagnostics.Append(diags...)
		m.Autoscaling = object
	}
	if pool.Status == nil {
		m.Phase = types.StringValue("")
		m.TotalCount = types.Int64Value(0)
		m.AvailableCount = types.Int64Value(0)
		m.ClaimedCount = types.Int64Value(0)
		return
	}
	m.Phase = types.StringValue(optionalString(pool.Status.Phase))
	m.TotalCount = types.Int64Value(optionalUint32(pool.Status.TotalCount))
	m.AvailableCount = types.Int64Value(optionalUint32(pool.Status.AvailableCount))
	m.ClaimedCount = types.Int64Value(optionalUint32(pool.Status.ClaimedCount))
}

func poolIdentity(namespace, name string) cyclops_sdk.Pool {
	return cyclops_sdk.Pool{Metadata: cyclops_sdk.ResourceMetadata{Namespace: namespace, Name: name}}
}

func isNotFound(err error) bool {
	var status *cyclops_sdk.SdkErrorStatus
	return errors.As(err, &status) && status.Status == http.StatusNotFound
}

func optionalUint32(value *uint32) int64 {
	if value == nil {
		return 0
	}
	return int64(*value)
}

func optionalString(value *string) string {
	if value == nil {
		return ""
	}
	return *value
}

func defaultString(value *string, fallback string) string {
	if value == nil || *value == "" {
		return fallback
	}
	return *value
}

func runtimeString(value *cyclops_sdk_schema.RuntimeKind) string {
	if value == nil || *value == cyclops_sdk_schema.RuntimeKindKubevirt {
		return "kubevirt"
	}
	if *value == cyclops_sdk_schema.RuntimeKindMacos {
		return "macos"
	}
	return "gvisor"
}

func firmwareString(value *cyclops_sdk_schema.Firmware) string {
	if value == nil || *value == cyclops_sdk_schema.FirmwareBios {
		return "bios"
	}
	return "efi"
}

func serviceProtocolString(value *cyclops_sdk_schema.ServiceProtocol) string {
	if value == nil || *value == cyclops_sdk_schema.ServiceProtocolTcp {
		return "TCP"
	}
	return "UDP"
}

func optionalServices(services *[]cyclops_sdk_schema.SandboxService) []cyclops_sdk_schema.SandboxService {
	if services == nil {
		return nil
	}
	return *services
}

func sdkProbes(value **cyclops_sdk_schema.PreservedJson, diagnostics *diag.Diagnostics) map[string]json.RawMessage {
	if value == nil || *value == nil {
		return nil
	}
	var probes map[string]json.RawMessage
	if err := json.Unmarshal([]byte((*value).ToJson()), &probes); err != nil {
		diagnostics.AddError("Unable to decode probe JSON", err.Error())
	}
	return probes
}

func decodeProbe(name string, value jsontypes.Normalized, probes map[string]json.RawMessage, diagnostics *diag.Diagnostics) {
	if value.IsNull() || value.IsUnknown() || value.ValueString() == "" {
		return
	}
	var decoded json.RawMessage
	if err := json.Unmarshal([]byte(value.ValueString()), &decoded); err != nil {
		diagnostics.AddError("Invalid probe JSON", err.Error())
		return
	}
	probes[name] = decoded
}

func encodeProbe(value json.RawMessage, diagnostics *diag.Diagnostics) jsontypes.Normalized {
	if value == nil {
		return jsontypes.NewNormalizedNull()
	}
	var decoded any
	if err := json.Unmarshal(value, &decoded); err != nil {
		diagnostics.AddError("Unable to encode probe JSON", err.Error())
		return jsontypes.NewNormalizedNull()
	}
	encoded, err := json.Marshal(decoded)
	if err != nil {
		diagnostics.AddError("Unable to encode probe JSON", err.Error())
		return jsontypes.NewNormalizedNull()
	}
	return jsontypes.NewNormalizedValue(string(encoded))
}

var _ resource.Resource = (*poolResource)(nil)
var _ resource.ResourceWithConfigure = (*poolResource)(nil)
var _ resource.ResourceWithImportState = (*poolResource)(nil)
