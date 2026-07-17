package client

import (
	"context"
	"net/http"
)

type Pool struct {
	APIVersion string       `json:"apiVersion,omitempty"`
	Kind       string       `json:"kind,omitempty"`
	Metadata   PoolMetadata `json:"metadata"`
	Spec       PoolSpec     `json:"spec"`
	Status     PoolStatus   `json:"status,omitempty"`
}

type PoolMetadata struct {
	Name      string            `json:"name"`
	Namespace string            `json:"namespace,omitempty"`
	Labels    map[string]string `json:"labels,omitempty"`
}

type PoolSpec struct {
	Replicas    int64        `json:"replicas"`
	Template    PoolTemplate `json:"template"`
	Services    []Service    `json:"services,omitempty"`
	Autoscaling *Autoscaling `json:"autoscaling,omitempty"`
}

type PoolTemplate struct {
	Runtime            string         `json:"runtime,omitempty"`
	ContainerDiskImage string         `json:"containerDiskImage"`
	ImagePullSecret    string         `json:"imagePullSecret,omitempty"`
	CPUCores           int64          `json:"cpuCores"`
	Memory             string         `json:"memory"`
	Firmware           string         `json:"firmware,omitempty"`
	Probes             map[string]any `json:"probes,omitempty"`
}

type Service struct {
	Name       string `json:"name"`
	TargetPort int64  `json:"targetPort"`
	Protocol   string `json:"protocol,omitempty"`
}

type Autoscaling struct {
	MinPoolSize     int64 `json:"minPoolSize"`
	InitialPoolSize int64 `json:"initialPoolSize"`
	MaxPoolSize     int64 `json:"maxPoolSize"`
}

type PoolStatus struct {
	Phase          string `json:"phase,omitempty"`
	TotalCount     int64  `json:"totalCount,omitempty"`
	AvailableCount int64  `json:"availableCount,omitempty"`
	ClaimedCount   int64  `json:"claimedCount,omitempty"`
}

func (c *Client) CreatePool(ctx context.Context, pool Pool) error {
	if err := c.do(ctx, http.MethodPost, "/api/namespaces", "", map[string]string{"name": pool.Metadata.Name}, nil); err != nil && !IsConflict(err) {
		return err
	}
	pool.APIVersion = "cua.ai/v1"
	pool.Kind = "OSGymWorkspacePool"
	pool.Metadata.Namespace = ""
	if pool.Metadata.Labels == nil {
		pool.Metadata.Labels = map[string]string{"cua.ai/pool": pool.Metadata.Name}
	}
	return c.do(ctx, http.MethodPost, poolCollectionPath(pool.Metadata.Name), "", pool, nil)
}

func (c *Client) GetPool(ctx context.Context, namespace, name string) (Pool, error) {
	var pool Pool
	err := c.do(ctx, http.MethodGet, poolPath(namespace, name), "", nil, &pool)
	return pool, err
}

func (c *Client) UpdatePool(ctx context.Context, namespace, name string, spec PoolSpec) error {
	return c.do(ctx, http.MethodPatch, poolPath(namespace, name), "application/merge-patch+json", map[string]any{"spec": spec}, nil)
}

func (c *Client) DeletePool(ctx context.Context, namespace, name string) error {
	if err := c.do(ctx, http.MethodDelete, poolPath(namespace, name), "", nil, nil); err != nil && !IsNotFound(err) {
		return err
	}
	if err := c.do(ctx, http.MethodDelete, "/api/namespaces/"+escape(namespace), "", nil, nil); err != nil && !IsNotFound(err) {
		return err
	}
	return nil
}

func poolCollectionPath(namespace string) string {
	return "/api/k8s/apis/cua.ai/v1/namespaces/" + escape(namespace) + "/osgymworkspacepools"
}

func poolPath(namespace, name string) string {
	return poolCollectionPath(namespace) + "/" + escape(name)
}
