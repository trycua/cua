// Code generated from clusters/base/osgym/crd.yaml; DO NOT EDIT.
package cyclopssdk

type ClaimSpec struct {
	BindDeadline       *uint32             `json:"bindDeadline,omitempty"`
	Lifecycle          *ClaimSpecLifecycle `json:"lifecycle,omitempty"`
	SandboxTemplateRef SandboxTemplateRef  `json:"sandboxTemplateRef"`
	Warmpool           *string             `json:"warmpool,omitempty"`
}

type ClaimSpecLifecycle struct {
	AutoRenew      *bool   `json:"autoRenew,omitempty"`
	ShutdownPolicy *string `json:"shutdownPolicy,omitempty"`
	ShutdownTime   *string `json:"shutdownTime,omitempty"`
}

type PoolSpec struct {
	Autoscaling *PoolSpecAutoscaling `json:"autoscaling,omitempty"`
	Replicas    uint32               `json:"replicas"`
	Services    []PoolSpecService    `json:"services,omitempty"`
	Template    PoolTemplate         `json:"template"`
}

type PoolSpecAutoscaling struct {
	InitialPoolSize *uint32 `json:"initialPoolSize,omitempty"`
	MaxPoolSize     *uint32 `json:"maxPoolSize,omitempty"`
	MinPoolSize     *uint32 `json:"minPoolSize,omitempty"`
}

type PoolSpecService struct {
	Name       string                   `json:"name"`
	Protocol   *PoolSpecServiceProtocol `json:"protocol,omitempty"`
	TargetPort uint32                   `json:"targetPort"`
}

type PoolSpecServiceProtocol string

const (
	PoolSpecServiceProtocolTCP PoolSpecServiceProtocol = "TCP"
	PoolSpecServiceProtocolUDP PoolSpecServiceProtocol = "UDP"
)

type PoolTemplate struct {
	Command            []string              `json:"command,omitempty"`
	ContainerDiskImage string                `json:"containerDiskImage"`
	CpuCores           *uint32               `json:"cpuCores,omitempty"`
	Firmware           *PoolTemplateFirmware `json:"firmware,omitempty"`
	ImagePullSecret    *string               `json:"imagePullSecret,omitempty"`
	Memory             *string               `json:"memory,omitempty"`
	NodeSelector       map[string]string     `json:"nodeSelector,omitempty"`
	Oidc               *PoolTemplateOidc     `json:"oidc,omitempty"`
	Probes             any                   `json:"probes,omitempty"`
	Runtime            *PoolTemplateRuntime  `json:"runtime,omitempty"`
	RuntimeClassName   *string               `json:"runtimeClassName,omitempty"`
	Tolerations        []any                 `json:"tolerations,omitempty"`
}

type PoolTemplateFirmware string

const (
	PoolTemplateFirmwareBios PoolTemplateFirmware = "bios"
	PoolTemplateFirmwareEfi  PoolTemplateFirmware = "efi"
)

type PoolTemplateOidc struct {
	AwsRegion              *string `json:"awsRegion,omitempty"`
	AwsRoleArn             *string `json:"awsRoleArn,omitempty"`
	CredentialsSecret      string  `json:"credentialsSecret"`
	RefreshIntervalSeconds *uint32 `json:"refreshIntervalSeconds,omitempty"`
	TokenUrl               string  `json:"tokenUrl"`
}

type PoolTemplateRuntime string

const (
	PoolTemplateRuntimeKubevirt PoolTemplateRuntime = "kubevirt"
	PoolTemplateRuntimeMacos    PoolTemplateRuntime = "macos"
	PoolTemplateRuntimeGvisor   PoolTemplateRuntime = "gvisor"
)

type SandboxTemplateRef struct {
	Name string `json:"name"`
}
