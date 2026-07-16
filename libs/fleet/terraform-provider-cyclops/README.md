# Terraform Provider for Cyclops

This provider manages `OSGymWorkspacePool` resources through the Cyclops API. It uses the same user-key OAuth credentials and tenant-scoped authorization as the Cyclops Python SDK and dashboard.

## Development

```bash
go test ./...
go build ./...

# Real Terraform protocol + kube-apiserver/CRD acceptance test
KUBEBUILDER_ASSETS="$(go run sigs.k8s.io/controller-runtime/tools/setup-envtest@release-0.19 use 1.31.0 -p path)" \
  TF_ACC=1 go test -tags=acceptance ./internal/provider -run TestAccPoolLifecycle -v
```

The provider source address is `trycua/cyclops`. A pool owns the same-named Cyclops namespace: create bootstraps the namespace, and destroy deletes the pool followed by that namespace.

## Authentication

Create a user API key in Cyclops and configure its returned `client_id`, `client_secret`, and `token_url`. For short-lived workflows, `access_token` can be supplied instead.

All provider arguments support environment variables:

- `CYCLOPS_ENDPOINT`
- `CYCLOPS_ACCESS_TOKEN`
- `CYCLOPS_CLIENT_ID`
- `CYCLOPS_CLIENT_SECRET`
- `CYCLOPS_TOKEN_URL`

## Pool resource generation

The Terraform models, schema, CRD-derived descriptions, enum validators, numeric validators, and nested object types are generated deterministically from:

- `clusters/base/osgym/crd.yaml`, the production `OSGymWorkspacePool` CRD
- `internal/provider/generate/pool_mapping.json`, the explicit CRD-to-Terraform shape mapping

From `cyclops-cs/terraform-provider-cyclops/`, run:

```bash
go generate ./...
```

`internal/provider/pool_generated.go` is committed. CI regenerates it and fails on drift. CRUD, namespace ownership, import behavior, default normalization, and API error handling remain handwritten in `internal/provider/pool_resource.go`.
