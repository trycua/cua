package handlers

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"slices"
	"strings"
	"time"

	"cyclops-cs-backend/identity"
)

const (
	capsuleTenantCollectionPath = "/apis/capsule.clastix.io/v1beta2/tenants"
	personalTenantEnsureTimeout = 15 * time.Second
	personalTenantReadyWaitStep = 250 * time.Millisecond
	k8sResponseBodyLimit        = 64 << 10
	tenantInformerWaitTimeout   = 5 * time.Second
	tenantInformerWaitStep      = 250 * time.Millisecond
)

var personalTenantClusterRoles = [...]string{
	"admin",
	"capsule-namespace-deleter",
	"capsule-tenant-cluster-resources",
}

type capsuleTenant struct {
	APIVersion string                `json:"apiVersion"`
	Kind       string                `json:"kind"`
	Metadata   capsuleTenantMetadata `json:"metadata"`
	Spec       capsuleTenantSpec     `json:"spec"`
	Status     *capsuleTenantStatus  `json:"status,omitempty"`
}

type capsuleTenantMetadata struct {
	Name string `json:"name"`
}

type capsuleTenantSpec struct {
	Owners                 []capsuleTenantOwner      `json:"owners"`
	Permissions            *capsuleTenantPermissions `json:"permissions,omitempty"`
	AdditionalRoleBindings []json.RawMessage         `json:"additionalRoleBindings,omitempty"`
}

type capsuleTenantPermissions struct {
	MatchOwners []json.RawMessage `json:"matchOwners,omitempty"`
}

type capsuleTenantStatus struct {
	Owners []capsuleTenantOwner `json:"owners,omitempty"`
}

type capsuleTenantOwner struct {
	Kind          string            `json:"kind"`
	Name          string            `json:"name"`
	ClusterRoles  []string          `json:"clusterRoles"`
	ProxySettings []json.RawMessage `json:"proxySettings,omitempty"`
}

func desiredPersonalTenant(ctx context.Context, userSub string) capsuleTenant {
	return capsuleTenant{
		APIVersion: "capsule.clastix.io/v1beta2",
		Kind:       "Tenant",
		Metadata: capsuleTenantMetadata{
			Name: identity.PersonalGroup(ctx, userSub),
		},
		Spec: capsuleTenantSpec{
			Owners: []capsuleTenantOwner{{
				Kind:         "User",
				Name:         identity.ImpersonateUser(ctx, userSub),
				ClusterRoles: append([]string(nil), personalTenantClusterRoles[:]...),
			}},
		},
	}
}

// k8sServiceAccount performs a direct Kubernetes apiserver request as the
// backend's ServiceAccount. Unlike k8sImpersonate, it bypasses Capsule Proxy
// and deliberately sends no Impersonate-* headers; personal Tenant lifecycle
// is a backend responsibility, not a capability delegated to the caller.
func (h Handlers) k8sServiceAccount(
	ctx context.Context, method, path string, body io.Reader,
) (*http.Response, error) {
	if k8sClient == nil {
		initK8sClient()
	}
	if k8sClient == nil {
		return nil, fmt.Errorf("k8s client not initialised (missing SA token)")
	}

	if k8sDirectAPIServer == "" {
		return nil, fmt.Errorf("direct k8s apiserver is not initialised")
	}
	req, err := http.NewRequestWithContext(ctx, method, k8sDirectAPIServer+path, body)
	if err != nil {
		return nil, fmt.Errorf("build request: %w", err)
	}
	req.Header.Set("Authorization", "Bearer "+k8sSAToken)
	req.Header.Set("Content-Type", "application/json")
	return k8sClient.Do(req)
}

// ensurePersonalTenant creates the authenticated user's Capsule Tenant on
// first use. Existing Tenants are accepted only when their single static owner
// matches the expected user, carries every required role, and dynamic owner
// matching is disabled. A mismatched object is never patched or replaced, so a
// name collision fails closed instead of changing ownership.
func (h Handlers) ensurePersonalTenant(ctx context.Context, userSub string) (string, error) {
	if userSub == "" {
		return "", fmt.Errorf("authenticated subject is empty")
	}

	ctx, cancel := context.WithTimeout(ctx, personalTenantEnsureTimeout)
	defer cancel()

	desired := desiredPersonalTenant(ctx, userSub)
	if !dnsLabel.MatchString(desired.Metadata.Name) || len(desired.Metadata.Name) > 63 {
		return "", fmt.Errorf("personal tenant name is not a DNS-1123 label")
	}

	exists, ready, err := h.getAndValidatePersonalTenant(ctx, desired)
	if err != nil {
		return "", err
	}
	if exists {
		if ready {
			return desired.Metadata.Name, nil
		}
		return h.waitForPersonalTenantReady(ctx, desired)
	}

	body, err := json.Marshal(desired)
	if err != nil {
		return "", fmt.Errorf("marshal personal tenant: %w", err)
	}
	resp, err := h.k8sServiceAccount(
		ctx, http.MethodPost, capsuleTenantCollectionPath, bytes.NewReader(body),
	)
	if err != nil {
		return "", fmt.Errorf("create personal tenant: %w", err)
	}
	if _, err := readBoundedK8sBody(resp.Body); err != nil {
		resp.Body.Close()
		return "", fmt.Errorf("create personal tenant response: %w", err)
	}
	resp.Body.Close()

	switch resp.StatusCode {
	case http.StatusCreated:
		// Admission webhooks can mutate a submitted object. Read the persisted
		// Tenant back and apply the same fail-closed ownership validation used
		// for pre-existing and concurrently-created objects. Capsule authorizes
		// namespace creation from status.owners, so also wait for the controller
		// to publish the exact effective owner before continuing.
		return h.waitForPersonalTenantReady(ctx, desired)
	case http.StatusConflict:
		// Another request may have created the same Tenant between our GET and
		// POST. Re-read it and verify ownership rather than trusting the 409.
		return h.waitForPersonalTenantReady(ctx, desired)
	default:
		return "", fmt.Errorf("create personal tenant: k8s returned status %d", resp.StatusCode)
	}
}

func (h Handlers) waitForPersonalTenantReady(
	ctx context.Context, desired capsuleTenant,
) (string, error) {
	for {
		exists, ready, err := h.getAndValidatePersonalTenant(ctx, desired)
		if err != nil {
			return "", err
		}
		if !exists {
			return "", fmt.Errorf("personal tenant was not found after creation")
		}
		if ready {
			return desired.Metadata.Name, nil
		}

		select {
		case <-time.After(personalTenantReadyWaitStep):
		case <-ctx.Done():
			return "", fmt.Errorf("wait for personal tenant readiness: %w", ctx.Err())
		}
	}
}

func (h Handlers) getAndValidatePersonalTenant(
	ctx context.Context, desired capsuleTenant,
) (bool, bool, error) {
	path := capsuleTenantCollectionPath + "/" + url.PathEscape(desired.Metadata.Name)
	resp, err := h.k8sServiceAccount(ctx, http.MethodGet, path, nil)
	if err != nil {
		return false, false, fmt.Errorf("get personal tenant: %w", err)
	}
	defer resp.Body.Close()
	body, err := readBoundedK8sBody(resp.Body)
	if err != nil {
		return false, false, fmt.Errorf("get personal tenant response: %w", err)
	}

	switch resp.StatusCode {
	case http.StatusNotFound:
		return false, false, nil
	case http.StatusOK:
		var existing capsuleTenant
		if err := json.Unmarshal(body, &existing); err != nil {
			return false, false, fmt.Errorf("decode existing personal tenant: %w", err)
		}
		if err := validatePersonalTenant(existing, desired); err != nil {
			return false, false, err
		}
		ready := existing.Status != nil && len(existing.Status.Owners) == 1
		return true, ready, nil
	default:
		return false, false, fmt.Errorf("get personal tenant: k8s returned status %d", resp.StatusCode)
	}
}

func validatePersonalTenant(existing, desired capsuleTenant) error {
	if existing.APIVersion != desired.APIVersion || existing.Kind != desired.Kind {
		return fmt.Errorf("existing personal tenant type does not match")
	}
	if existing.Metadata.Name != desired.Metadata.Name {
		return fmt.Errorf("existing personal tenant metadata does not match")
	}
	if len(existing.Spec.Owners) != 1 {
		return fmt.Errorf("existing personal tenant owner does not match")
	}
	// Capsule promotes TenantOwner resources selected by matchOwners into the
	// effective owner list. Personal Tenants must never enable that second
	// ownership path, even when their static owner looks correct.
	if existing.Spec.Permissions != nil &&
		len(existing.Spec.Permissions.MatchOwners) != 0 {
		return fmt.Errorf("existing personal tenant owner does not match")
	}
	if len(existing.Spec.AdditionalRoleBindings) != 0 {
		return fmt.Errorf("existing personal tenant role bindings do not match")
	}
	existingOwner := existing.Spec.Owners[0]
	desiredOwner := desired.Spec.Owners[0]
	if existingOwner.Kind != desiredOwner.Kind ||
		existingOwner.Name != desiredOwner.Name ||
		!sameClusterRoleSet(existingOwner.ClusterRoles, desiredOwner.ClusterRoles) ||
		len(existingOwner.ProxySettings) != 0 {
		return fmt.Errorf("existing personal tenant owner does not match")
	}
	// status.owners is what Capsule uses for authorization. It may be empty
	// immediately after creation, before the controller's first reconcile. Once
	// populated, reject stale or dynamically-added effective owners as well.
	if existing.Status != nil && len(existing.Status.Owners) != 0 {
		if len(existing.Status.Owners) != 1 {
			return fmt.Errorf("existing personal tenant effective owner does not match")
		}
		effectiveOwner := existing.Status.Owners[0]
		if effectiveOwner.Kind != desiredOwner.Kind ||
			effectiveOwner.Name != desiredOwner.Name ||
			!sameClusterRoleSet(effectiveOwner.ClusterRoles, desiredOwner.ClusterRoles) {
			return fmt.Errorf("existing personal tenant effective owner does not match")
		}
	}
	return nil
}

func sameClusterRoleSet(existing, required []string) bool {
	if len(existing) != len(required) {
		return false
	}
	for _, role := range required {
		if !slices.Contains(existing, role) {
			return false
		}
	}
	return true
}

func readBoundedK8sBody(r io.Reader) ([]byte, error) {
	body, err := io.ReadAll(io.LimitReader(r, k8sResponseBodyLimit+1))
	if err != nil {
		return nil, err
	}
	if len(body) > k8sResponseBodyLimit {
		return nil, fmt.Errorf("response body exceeds %d bytes", k8sResponseBodyLimit)
	}
	return body, nil
}

// isPersonalTenantAdmissionCacheLag identifies the two retryable namespace
// admission failures after a Tenant is first created. Capsule's webhook reads
// both the Tenant and status.owners from its controller-runtime cache, which
// can briefly lag the successful direct-apiserver reads above. Match only the
// exact Capsule errors so unrelated admission failures return immediately.
func isPersonalTenantAdmissionCacheLag(statusCode int, body []byte, tenantName string) bool {
	var status struct {
		Kind    string `json:"kind"`
		Status  string `json:"status"`
		Message string `json:"message"`
		Code    int    `json:"code"`
	}
	if json.Unmarshal(body, &status) != nil ||
		status.Kind != "Status" ||
		status.Status != "Failure" ||
		status.Code != statusCode {
		return false
	}
	if statusCode == http.StatusBadRequest {
		return strings.Contains(
			status.Message,
			"can not assign the desired namespace to a non-owned Tenant",
		)
	}
	if statusCode != http.StatusInternalServerError {
		return false
	}
	for _, resource := range []string{
		"Tenant.capsule.clastix.io",
		"tenants.capsule.clastix.io",
	} {
		want := fmt.Sprintf("%s %q not found", resource, tenantName)
		if strings.Contains(status.Message, want) {
			return true
		}
	}
	return false
}
