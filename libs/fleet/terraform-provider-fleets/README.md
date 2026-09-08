# Terraform Provider for Cua Fleets

This provider manages `OSGymSandboxWarmPool` and `OSGymSandboxTemplate` resources through the Fleet API. It uses the same user-key OAuth credentials and tenant-scoped authorization as the Fleet Python SDK and dashboard.

## Installation

Declare the provider in your Terraform configuration:

```hcl
terraform {
  required_providers {
    fleets = {
      source  = "trycua/fleets"
      version = "0.2.0"
    }
  }
}
```

Then install it from the Terraform Registry:

```bash
terraform init
```

## Development

```bash
native="$(../scripts/build-sdk-bindings-native.sh)"
native_dir="$(dirname "$native")"
export CGO_CFLAGS="-I../sdk-bindings/go-uniffi/fleet_sdk -I../sdk-bindings/go-uniffi/cyclops_sdk_schema"
export CGO_LDFLAGS="-L$native_dir -lcyclops_sdk -Wl,-rpath,$native_dir"
export LD_LIBRARY_PATH="$native_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

go test ./...
go build ./...

# Real Terraform protocol + kube-apiserver/CRD acceptance test
KUBEBUILDER_ASSETS="$(go run sigs.k8s.io/controller-runtime/tools/setup-envtest@release-0.19 use 1.31.0 -p path)" \
  TF_ACC=1 go test -tags=acceptance ./internal/provider -run TestAccPoolLifecycle -v
```

The provider source address is `trycua/fleets`. One `fleets_pool` is a pair of native CRs in the same namespace: an `OSGymSandboxWarmPool` named after the pool and the `OSGymSandboxTemplate` it references, named `<pool>-template`. A pool owns the same-named Fleet namespace: creating the warm pool bootstraps the namespace before the template is written into it, and destroy deletes the template, then the pool, then that namespace.

## Authentication

Create a user API key in Cua and configure its returned `client_id`, `client_secret`, and `token_url`. For short-lived workflows, `access_token` can be supplied instead.

All provider arguments support environment variables:

- `CYCLOPS_ENDPOINT`
- `CYCLOPS_ACCESS_TOKEN`
- `CYCLOPS_CLIENT_ID`
- `CYCLOPS_CLIENT_SECRET`
- `CYCLOPS_TOKEN_URL`

## Pool resource generation

The Terraform models, schema, CRD-derived descriptions, enum validators, numeric validators, and nested object types are generated deterministically from:

- `clusters/base/osgym/crd.yaml`, the production CRD bundle; the generator reads the `v1alpha1` schemas of `osgymsandboxwarmpools.osgym.cua.ai` and `osgymsandboxtemplates.osgym.cua.ai`
- `internal/provider/generate/pool_mapping.json`, the explicit CRD-to-Terraform shape mapping. Every mapped attribute names the CR it lives in with `"cr": "warmpool"` or `"cr": "template"`

Run:

```bash
go generate ./...
```

`internal/provider/pool_generated.go` is committed. CI regenerates it and fails on drift. CRUD, namespace ownership, import behavior, default normalization, and API error handling remain handwritten in `internal/provider/pool_resource.go`.
