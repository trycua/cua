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
	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/cyclops_sdk_schema"
	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/fleet_sdk"
)

// The flat resource is backed by an OSGymSandboxWarmPool and the
// OSGymSandboxTemplate it references, both named after the pool.
const templateNameSuffix = "-template"

type poolResource struct {
	client     *fleet_sdk.CyclopsClient
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
	apiClient, ok := req.ProviderData.(*fleet_sdk.CyclopsClient)
	if !ok {
		resp.Diagnostics.AddError("Unexpected provider data", fmt.Sprintf("expected *fleet_sdk.CyclopsClient, got %T", req.ProviderData))
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
	poolRequest := plan.toSDKCreatePoolRequest(ctx, &resp.Diagnostics)
	templateRequest := plan.toSDKCreateTemplateRequest(ctx, &resp.Diagnostics)
	if resp.Diagnostics.HasError() {
		return
	}
	// The warm pool owns the namespace both objects live in, so it has to exist
	// before the template can be posted into that namespace.
	created, err := r.client.CreatePool(poolRequest)
	if err != nil {
		resp.Diagnostics.AddError("Unable to create Fleet pool", err.Error())
		return
	}
	if _, err := r.client.CreateTemplate(templateRequest); err != nil {
		if deleteErr := r.client.DeletePool(created); deleteErr != nil {
			resp.Diagnostics.AddWarning("Fleet pool was left behind", deleteErr.Error())
		}
		resp.Diagnostics.AddError("Unable to create Fleet sandbox template", err.Error())
		return
	}
	r.refresh(ctx, &plan, "Pool created but could not be read", &resp.Diagnostics)
	if resp.Diagnostics.HasError() {
		return
	}
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
		resp.Diagnostics.AddError("Unable to read Fleet pool", err.Error())
		return
	}
	template, err := r.getTemplate(pool)
	if err != nil {
		resp.Diagnostics.AddError("Unable to read Fleet sandbox template", err.Error())
		return
	}
	if template == nil {
		resp.Diagnostics.AddWarning(
			"Fleet sandbox template is missing",
			fmt.Sprintf("%s/%s no longer exists; applying this resource recreates it.", pool.Metadata.Namespace, pool.Spec.SandboxTemplateRef.Name),
		)
	}
	state.fromSDKPool(pool, &resp.Diagnostics)
	state.fromSDKTemplate(ctx, template, &resp.Diagnostics)
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *poolResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan, state poolResourceModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if !plan.warmPoolAttributesEqual(state) {
		pool := plan.toSDKPool(ctx, &resp.Diagnostics)
		if resp.Diagnostics.HasError() {
			return
		}
		if _, err := r.client.UpdatePool(pool); err != nil {
			resp.Diagnostics.AddError("Unable to update Fleet pool", err.Error())
			return
		}
	}
	if !plan.templateAttributesEqual(state) {
		// Reconcile rather than patch so a template deleted out of band is
		// restored instead of failing the apply.
		if _, err := r.client.ReconcileTemplate(plan.toSDKCreateTemplateRequest(ctx, &resp.Diagnostics)); err != nil {
			resp.Diagnostics.AddError("Unable to update Fleet sandbox template", err.Error())
			return
		}
	}
	if resp.Diagnostics.HasError() {
		return
	}
	r.refresh(ctx, &plan, "Pool updated but could not be read", &resp.Diagnostics)
	if resp.Diagnostics.HasError() {
		return
	}
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *poolResource) Delete(ctx context.Context, req resource.DeleteRequest, resp *resource.DeleteResponse) {
	var state poolResourceModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	namespace := state.Namespace.ValueString()
	if err := r.client.DeleteTemplate(templateIdentity(namespace, state.templateName())); err != nil {
		resp.Diagnostics.AddError("Unable to delete Fleet sandbox template", err.Error())
		return
	}
	// Deleting the pool also deletes the namespace the template lived in.
	if err := r.client.DeletePool(poolIdentity(namespace, state.Name.ValueString())); err != nil {
		resp.Diagnostics.AddError("Unable to delete Fleet pool", err.Error())
	}
}

func (r *poolResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("id"), req.ID)...)
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("name"), req.ID)...)
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("namespace"), req.ID)...)
}

// refresh reloads both objects into the model after a write.
func (r *poolResource) refresh(ctx context.Context, model *poolResourceModel, failure string, diagnostics *diag.Diagnostics) {
	pool, err := r.client.GetPool(model.Name.ValueString())
	if err != nil {
		diagnostics.AddError(failure, err.Error())
		return
	}
	template, err := r.getTemplate(pool)
	if err != nil {
		diagnostics.AddError(failure, err.Error())
		return
	}
	model.fromSDKPool(pool, diagnostics)
	model.fromSDKTemplate(ctx, template, diagnostics)
}

// getTemplate returns nil when the template a pool references is gone.
func (r *poolResource) getTemplate(pool fleet_sdk.Pool) (*fleet_sdk.Template, error) {
	template, err := r.client.GetTemplate(pool.Metadata.Namespace, pool.Spec.SandboxTemplateRef.Name)
	if isNotFound(err) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return &template, nil
}

func (m poolResourceModel) toSDKCreatePoolRequest(ctx context.Context, diagnostics *diag.Diagnostics) fleet_sdk.CreatePoolRequest {
	return fleet_sdk.CreatePoolRequest{Namespace: m.Name.ValueString(), Spec: m.toSDKPoolSpec(ctx, diagnostics)}
}

func (m poolResourceModel) toSDKCreateTemplateRequest(ctx context.Context, diagnostics *diag.Diagnostics) fleet_sdk.CreateTemplateRequest {
	return fleet_sdk.CreateTemplateRequest{
		Namespace: m.Name.ValueString(), Name: m.templateName(), Spec: m.toSDKTemplateSpec(ctx, diagnostics),
	}
}

func (m poolResourceModel) toSDKPool(ctx context.Context, diagnostics *diag.Diagnostics) fleet_sdk.Pool {
	name := m.Name.ValueString()
	return fleet_sdk.Pool{
		ApiVersion: "osgym.cua.ai/v1alpha1", Kind: "OSGymSandboxWarmPool",
		Metadata: fleet_sdk.ResourceMetadata{Namespace: name, Name: name},
		Spec:     m.toSDKPoolSpec(ctx, diagnostics),
	}
}

func (m poolResourceModel) toSDKPoolSpec(ctx context.Context, diagnostics *diag.Diagnostics) cyclops_sdk_schema.OsGymSandboxWarmPoolSpec {
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
	return cyclops_sdk_schema.OsGymSandboxWarmPoolSpec{
		Replicas:           uint32(m.Replicas.ValueInt64()),
		SandboxTemplateRef: cyclops_sdk_schema.SandboxTemplateRef{Name: m.templateName()},
		Autoscaling:        autoscaling,
	}
}

func (m poolResourceModel) toSDKTemplateSpec(ctx context.Context, diagnostics *diag.Diagnostics) cyclops_sdk_schema.OsGymSandboxTemplateSpec {
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
	return cyclops_sdk_schema.OsGymSandboxTemplateSpec{
		VmTemplate: cyclops_sdk_schema.VmTemplate{
			Runtime: runtime, ContainerDiskImage: m.ContainerDiskImage.ValueString(), ImagePullSecret: &imagePullSecret,
			CpuCores: &cpuCores, Memory: &memory, Firmware: firmware, Probes: probes, Services: &services,
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

// templateName is the OSGymSandboxTemplate the warm pool references. It is
// computed, so a plan that has not read it back yet falls back to the name the
// provider assigns.
func (m poolResourceModel) templateName() string {
	if name := m.TemplateName.ValueString(); name != "" {
		return name
	}
	return m.Name.ValueString() + templateNameSuffix
}

func (m poolResourceModel) warmPoolAttributesEqual(other poolResourceModel) bool {
	return m.Replicas.Equal(other.Replicas) &&
		m.TemplateName.Equal(other.TemplateName) &&
		m.Autoscaling.Equal(other.Autoscaling)
}

func (m poolResourceModel) templateAttributesEqual(other poolResourceModel) bool {
	return m.CPUCores.Equal(other.CPUCores) &&
		m.Memory.Equal(other.Memory) &&
		m.ContainerDiskImage.Equal(other.ContainerDiskImage) &&
		m.ImagePullSecret.Equal(other.ImagePullSecret) &&
		m.Runtime.Equal(other.Runtime) &&
		m.Firmware.Equal(other.Firmware) &&
		m.ReadinessProbeJSON.Equal(other.ReadinessProbeJSON) &&
		m.LivenessProbeJSON.Equal(other.LivenessProbeJSON) &&
		m.Services.Equal(other.Services)
}

func (m *poolResourceModel) fromSDKPool(pool fleet_sdk.Pool, diagnostics *diag.Diagnostics) {
	m.ID = types.StringValue(pool.Metadata.Name)
	m.Name = types.StringValue(pool.Metadata.Name)
	m.Namespace = types.StringValue(pool.Metadata.Namespace)
	m.Replicas = types.Int64Value(int64(pool.Spec.Replicas))
	m.TemplateName = types.StringValue(pool.Spec.SandboxTemplateRef.Name)
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
		m.CurrentReplicas = types.Int64Value(0)
		m.ReadyReplicas = types.Int64Value(0)
		return
	}
	m.CurrentReplicas = types.Int64Value(optionalUint32(pool.Status.Replicas))
	m.ReadyReplicas = types.Int64Value(optionalUint32(pool.Status.ReadyReplicas))
}

// fromSDKTemplate reads the template-owned attributes. A nil template — the
// warm pool referencing one that no longer exists — clears them so the next
// plan restores the template.
func (m *poolResourceModel) fromSDKTemplate(ctx context.Context, template *fleet_sdk.Template, diagnostics *diag.Diagnostics) {
	vmTemplate := cyclops_sdk_schema.VmTemplate{}
	if template != nil {
		vmTemplate = template.Spec.VmTemplate
	}
	m.CPUCores = types.Int64Value(optionalUint32(vmTemplate.CpuCores))
	m.Memory = types.StringValue(optionalString(vmTemplate.Memory))
	m.ContainerDiskImage = types.StringValue(vmTemplate.ContainerDiskImage)
	m.ImagePullSecret = types.StringValue(defaultString(vmTemplate.ImagePullSecret, "ecr-credentials"))
	m.Runtime = types.StringValue(runtimeString(vmTemplate.Runtime))
	m.Firmware = types.StringValue(firmwareString(vmTemplate.Firmware))
	probes := sdkProbes(vmTemplate.Probes, diagnostics)
	m.ReadinessProbeJSON = encodeProbe(probes["readinessProbe"], diagnostics)
	m.LivenessProbeJSON = encodeProbe(probes["livenessProbe"], diagnostics)
	serviceType := serviceObjectType()
	serviceValues := make([]attr.Value, 0, len(optionalServices(vmTemplate.Services)))
	for _, value := range optionalServices(vmTemplate.Services) {
		object, diags := types.ObjectValue(serviceType, map[string]attr.Value{
			"name": types.StringValue(value.Name), "target_port": types.Int64Value(int64(value.TargetPort)), "protocol": types.StringValue(serviceProtocolString(value.Protocol)),
		})
		diagnostics.Append(diags...)
		serviceValues = append(serviceValues, object)
	}
	services, diags := types.SetValue(types.ObjectType{AttrTypes: serviceType}, serviceValues)
	diagnostics.Append(diags...)
	m.Services = services
}

func poolIdentity(namespace, name string) fleet_sdk.Pool {
	return fleet_sdk.Pool{Metadata: fleet_sdk.ResourceMetadata{Namespace: namespace, Name: name}}
}

func templateIdentity(namespace, name string) fleet_sdk.Template {
	return fleet_sdk.Template{Metadata: fleet_sdk.ResourceMetadata{Namespace: namespace, Name: name}}
}

func isNotFound(err error) bool {
	var status *fleet_sdk.SdkErrorStatus
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
