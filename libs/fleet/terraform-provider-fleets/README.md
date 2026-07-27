# Terraform Provider for Cua Fleets

This provider manages `OSGymWorkspacePool` resources through the Fleet API. It uses the same user-key OAuth credentials and tenant-scoped authorization as the Fleet Python SDK and dashboard.

## Temporary local installation (until Registry publication)

Terraform and OpenTofu provider source addresses are registry addresses, not
GitHub URLs. Until `trycua/fleets` is published to the Terraform Registry,
build the public repository locally and install it through a filesystem mirror.
This installs a locally built binary; it does **not** install the provider from
the public Registry. Replace or remove this section after Registry publication.

The intended first provider version is `1.0.0`. Clone the public repository and
build the provider for your current operating system and architecture:

```bash
git clone https://github.com/trycua/terraform-provider-fleets.git
cd terraform-provider-fleets

VERSION=1.0.0
OS="$(go env GOOS)"
ARCH="$(go env GOARCH)"
MIRROR="$HOME/.terraform.d/plugins"
TARGET="$MIRROR/registry.terraform.io/trycua/fleets/$VERSION/${OS}_${ARCH}"

mkdir -p "$TARGET"
go build -o "$TARGET/terraform-provider-fleets_v${VERSION}" .
```

Create a Terraform CLI configuration file that uses that mirror and explicitly
prevents a Registry lookup for this provider:

```bash
CLI_CONFIG="$HOME/.terraformrc-fleets-local"
cat > "$CLI_CONFIG" <<EOF
provider_installation {
  filesystem_mirror {
    path    = "$MIRROR"
    include = ["registry.terraform.io/trycua/fleets"]
  }

  direct {
    exclude = ["registry.terraform.io/trycua/fleets"]
  }
}
EOF

export TF_CLI_CONFIG_FILE="$CLI_CONFIG"
```

On Windows PowerShell, build the matching Windows binary and write the same
CLI configuration with an absolute mirror path:

```powershell
git clone https://github.com/trycua/terraform-provider-fleets.git
Set-Location terraform-provider-fleets

$Version = "1.0.0"
$Os = go env GOOS
$Arch = go env GOARCH
$Mirror = Join-Path $HOME ".terraform.d\plugins"
$Target = Join-Path $Mirror "registry.terraform.io\trycua\fleets\$Version\${Os}_${Arch}"
New-Item -ItemType Directory -Force -Path $Target | Out-Null
go build -o (Join-Path $Target "terraform-provider-fleets_v$Version.exe") .

$CliConfig = Join-Path $HOME "terraformrc-fleets-local.tfrc"
@"
provider_installation {
  filesystem_mirror {
    path    = "$($Mirror -replace '\\', '/')"
    include = ["registry.terraform.io/trycua/fleets"]
  }

  direct {
    exclude = ["registry.terraform.io/trycua/fleets"]
  }
}
"@ | Set-Content -NoNewline $CliConfig

$env:TF_CLI_CONFIG_FILE = $CliConfig
```

Use the normal provider address and version in configuration. Do not replace
`source` with a GitHub URL:

```hcl
terraform {
  required_providers {
    fleets = {
      source  = "trycua/fleets"
      version = "1.0.0"
    }
  }
}
```

With `TF_CLI_CONFIG_FILE` set, remove any prior initialization state and run
Terraform (or substitute `tofu` for OpenTofu):

```bash
rm -rf .terraform .terraform.lock.hcl
terraform init
terraform providers
terraform providers schema -json >/dev/null
```

`terraform init` should succeed without resolving `trycua/fleets` from the
public Registry, and `terraform providers` should list
`provider[registry.terraform.io/trycua/fleets]` at version `1.0.0`.

## Development

```bash
go test ./...
go build ./...

# Real Terraform protocol + kube-apiserver/CRD acceptance test
KUBEBUILDER_ASSETS="$(go run sigs.k8s.io/controller-runtime/tools/setup-envtest@release-0.19 use 1.31.0 -p path)" \
  TF_ACC=1 go test -tags=acceptance ./internal/provider -run TestAccPoolLifecycle -v
```

The provider source address is `trycua/fleets`. A pool owns the same-named Fleet namespace: create bootstraps the namespace, and destroy deletes the pool followed by that namespace.

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

- `clusters/base/osgym/crd.yaml`, the production `OSGymWorkspacePool` CRD
- `internal/provider/generate/pool_mapping.json`, the explicit CRD-to-Terraform shape mapping

From `cyclops-cs/terraform-provider-fleets/`, run:

```bash
go generate ./...
```

`internal/provider/pool_generated.go` is committed. CI regenerates it and fails on drift. CRUD, namespace ownership, import behavior, default normalization, and API error handling remain handwritten in `internal/provider/pool_resource.go`.
