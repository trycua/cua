package provider

//go:generate go run ../../cmd/generate-pool-resource -crd ../../../../clusters/base/osgym/crd.yaml -mapping generate/pool_mapping.json -out pool_generated.go

import (
	"context"
	"encoding/json"
	"fmt"

	"github.com/hashicorp/terraform-plugin-framework-jsontypes/jsontypes"
	"github.com/hashicorp/terraform-plugin-framework/attr"
	"github.com/hashicorp/terraform-plugin-framework/diag"
	"github.com/hashicorp/terraform-plugin-framework/path"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/types"
	"github.com/trycua/terraform-provider-cyclops/internal/client"
)

type poolResource struct{ client *client.Client }

func NewPoolResource() resource.Resource { return &poolResource{} }

func (r *poolResource) Metadata(_ context.Context, req resource.MetadataRequest, resp *resource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_pool"
}

func (r *poolResource) Schema(_ context.Context, _ resource.SchemaRequest, resp *resource.SchemaResponse) {
	resp.Schema = poolResourceSchema()
}

func (r *poolResource) Configure(_ context.Context, req resource.ConfigureRequest, resp *resource.ConfigureResponse) {
	if req.ProviderData == nil {
		return
	}
	apiClient, ok := req.ProviderData.(*client.Client)
	if !ok {
		resp.Diagnostics.AddError("Unexpected provider data", fmt.Sprintf("expected *client.Client, got %T", req.ProviderData))
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
	pool := plan.toPool(ctx, &resp.Diagnostics)
	if resp.Diagnostics.HasError() {
		return
	}
	if err := r.client.CreatePool(ctx, pool); err != nil {
		resp.Diagnostics.AddError("Unable to create Cyclops pool", err.Error())
		return
	}
	created, err := r.client.GetPool(ctx, plan.Name.ValueString(), plan.Name.ValueString())
	if err != nil {
		resp.Diagnostics.AddError("Pool created but could not be read", err.Error())
		return
	}
	plan.fromPool(ctx, created, &resp.Diagnostics)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *poolResource) Read(ctx context.Context, req resource.ReadRequest, resp *resource.ReadResponse) {
	var state poolResourceModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	pool, err := r.client.GetPool(ctx, state.Namespace.ValueString(), state.Name.ValueString())
	if client.IsNotFound(err) {
		resp.State.RemoveResource(ctx)
		return
	}
	if err != nil {
		resp.Diagnostics.AddError("Unable to read Cyclops pool", err.Error())
		return
	}
	state.fromPool(ctx, pool, &resp.Diagnostics)
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *poolResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan poolResourceModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	pool := plan.toPool(ctx, &resp.Diagnostics)
	if resp.Diagnostics.HasError() {
		return
	}
	namespace := plan.Name.ValueString()
	if err := r.client.UpdatePool(ctx, namespace, plan.Name.ValueString(), pool.Spec); err != nil {
		resp.Diagnostics.AddError("Unable to update Cyclops pool", err.Error())
		return
	}
	updated, err := r.client.GetPool(ctx, namespace, plan.Name.ValueString())
	if err != nil {
		resp.Diagnostics.AddError("Pool updated but could not be read", err.Error())
		return
	}
	plan.fromPool(ctx, updated, &resp.Diagnostics)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *poolResource) Delete(ctx context.Context, req resource.DeleteRequest, resp *resource.DeleteResponse) {
	var state poolResourceModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if err := r.client.DeletePool(ctx, state.Namespace.ValueString(), state.Name.ValueString()); err != nil {
		resp.Diagnostics.AddError("Unable to delete Cyclops pool", err.Error())
	}
}

func (r *poolResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("id"), req.ID)...)
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("name"), req.ID)...)
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("namespace"), req.ID)...)
}

func (m poolResourceModel) toPool(ctx context.Context, diagnostics *diag.Diagnostics) client.Pool {
	services := []client.Service{}
	if !m.Services.IsNull() && !m.Services.IsUnknown() {
		var values []serviceModel
		diagnostics.Append(m.Services.ElementsAs(ctx, &values, false)...)
		for _, value := range values {
			protocol := value.Protocol.ValueString()
			if protocol == "" {
				protocol = "TCP"
			}
			services = append(services, client.Service{Name: value.Name.ValueString(), TargetPort: value.TargetPort.ValueInt64(), Protocol: protocol})
		}
	}
	var autoscaling *client.Autoscaling
	var autoscalingValue autoscalingModel
	if objectValue(ctx, m.Autoscaling, &autoscalingValue, diagnostics) {
		autoscaling = &client.Autoscaling{MinPoolSize: autoscalingValue.MinPoolSize.ValueInt64(), InitialPoolSize: autoscalingValue.InitialPoolSize.ValueInt64(), MaxPoolSize: autoscalingValue.MaxPoolSize.ValueInt64()}
	}
	probes := map[string]any{}
	decodeProbe("readinessProbe", m.ReadinessProbeJSON, probes, diagnostics)
	decodeProbe("livenessProbe", m.LivenessProbeJSON, probes, diagnostics)
	if len(probes) == 0 {
		probes = nil
	}
	runtime := m.Runtime.ValueString()
	if runtime == "kubevirt" {
		runtime = ""
	}
	firmware := m.Firmware.ValueString()
	if firmware == "bios" {
		firmware = ""
	}
	imagePullSecret := m.ImagePullSecret.ValueString()
	if imagePullSecret == "" {
		imagePullSecret = "ecr-credentials"
	}
	return client.Pool{Metadata: client.PoolMetadata{Name: m.Name.ValueString()}, Spec: client.PoolSpec{
		Replicas: m.Replicas.ValueInt64(), Services: services, Autoscaling: autoscaling,
		Template: client.PoolTemplate{Runtime: runtime, ContainerDiskImage: m.ContainerDiskImage.ValueString(), ImagePullSecret: imagePullSecret, CPUCores: m.CPUCores.ValueInt64(), Memory: m.Memory.ValueString(), Firmware: firmware, Probes: probes},
	}}
}

func (m *poolResourceModel) fromPool(ctx context.Context, pool client.Pool, diagnostics *diag.Diagnostics) {
	m.ID = types.StringValue(pool.Metadata.Name)
	m.Name = types.StringValue(pool.Metadata.Name)
	m.Namespace = types.StringValue(pool.Metadata.Namespace)
	m.Replicas = types.Int64Value(pool.Spec.Replicas)
	m.CPUCores = types.Int64Value(pool.Spec.Template.CPUCores)
	m.Memory = types.StringValue(pool.Spec.Template.Memory)
	m.ContainerDiskImage = types.StringValue(pool.Spec.Template.ContainerDiskImage)
	imagePullSecret := pool.Spec.Template.ImagePullSecret
	if imagePullSecret == "" {
		imagePullSecret = "ecr-credentials"
	}
	m.ImagePullSecret = types.StringValue(imagePullSecret)
	runtime := pool.Spec.Template.Runtime
	if runtime == "" {
		runtime = "kubevirt"
	}
	m.Runtime = types.StringValue(runtime)
	firmware := pool.Spec.Template.Firmware
	if firmware == "" {
		firmware = "bios"
	}
	m.Firmware = types.StringValue(firmware)
	m.ReadinessProbeJSON = encodeProbe(pool.Spec.Template.Probes["readinessProbe"], diagnostics)
	m.LivenessProbeJSON = encodeProbe(pool.Spec.Template.Probes["livenessProbe"], diagnostics)
	serviceType := serviceObjectType()
	serviceValues := make([]attr.Value, 0, len(pool.Spec.Services))
	for _, value := range pool.Spec.Services {
		protocol := value.Protocol
		if protocol == "" {
			protocol = "TCP"
		}
		object, diags := types.ObjectValue(serviceType, map[string]attr.Value{"name": types.StringValue(value.Name), "target_port": types.Int64Value(value.TargetPort), "protocol": types.StringValue(protocol)})
		diagnostics.Append(diags...)
		serviceValues = append(serviceValues, object)
	}
	services, diags := types.SetValue(types.ObjectType{AttrTypes: serviceType}, serviceValues)
	diagnostics.Append(diags...)
	m.Services = services
	if pool.Spec.Autoscaling == nil {
		m.Autoscaling = types.ObjectNull(autoscalingObjectType())
	} else {
		object, diags := types.ObjectValue(autoscalingObjectType(), map[string]attr.Value{"min_pool_size": types.Int64Value(pool.Spec.Autoscaling.MinPoolSize), "initial_pool_size": types.Int64Value(pool.Spec.Autoscaling.InitialPoolSize), "max_pool_size": types.Int64Value(pool.Spec.Autoscaling.MaxPoolSize)})
		diagnostics.Append(diags...)
		m.Autoscaling = object
	}
	m.Phase = types.StringValue(pool.Status.Phase)
	m.TotalCount = types.Int64Value(pool.Status.TotalCount)
	m.AvailableCount = types.Int64Value(pool.Status.AvailableCount)
	m.ClaimedCount = types.Int64Value(pool.Status.ClaimedCount)
}

func decodeProbe(name string, value jsontypes.Normalized, probes map[string]any, diagnostics *diag.Diagnostics) {
	if value.IsNull() || value.IsUnknown() || value.ValueString() == "" {
		return
	}
	var decoded any
	if err := json.Unmarshal([]byte(value.ValueString()), &decoded); err != nil {
		diagnostics.AddError("Invalid probe JSON", err.Error())
		return
	}
	probes[name] = decoded
}
func encodeProbe(value any, diagnostics *diag.Diagnostics) jsontypes.Normalized {
	if value == nil {
		return jsontypes.NewNormalizedNull()
	}
	encoded, err := json.Marshal(value)
	if err != nil {
		diagnostics.AddError("Unable to encode probe JSON", err.Error())
		return jsontypes.NewNormalizedNull()
	}
	return jsontypes.NewNormalizedValue(string(encoded))
}

var _ resource.Resource = (*poolResource)(nil)
var _ resource.ResourceWithConfigure = (*poolResource)(nil)
var _ resource.ResourceWithImportState = (*poolResource)(nil)
