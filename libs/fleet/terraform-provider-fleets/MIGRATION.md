# Migrating from the Cyclops provider

`trycua/fleets` is the public Terraform Registry provider. The Cloud service
APIs and OAuth environment variables remain unchanged. `fleets_pool` now backs
each pool with the native `osgym.cua.ai/v1alpha1` CRDs — an
`OSGymSandboxWarmPool` and the `OSGymSandboxTemplate` it references — in place
of the removed legacy pool CRD. Apart from the pool sizing-mode change
described below, the resource arguments for this Cyclops-to-Fleets migration
are unchanged; the `phase`, `total_count`, `available_count`, and
`claimed_count` attributes it used to export are replaced by `template_name`,
`current_replicas`, and `ready_replicas`.

| Previous surface | Fleets surface | Migration |
| --- | --- | --- |
| `trycua/cyclops` | `trycua/fleets` | Change `required_providers`; Terraform provider source addresses cannot be aliased by a provider binary. |
| `provider "cyclops"` | `provider "fleets"` | Rename the provider configuration and references to it. |
| `cyclops_pool` | `fleets_pool` | Rename new configuration. `cyclops_pool` remains a state-compatibility alias. |
| `terraform-provider-cyclops` | `terraform-provider-fleets` | Update development and CI binary paths. |
| `github.com/trycua/terraform-provider-cyclops` | `github.com/trycua/terraform-provider-fleets` | Update Go module imports. |

For existing state, update configuration to `fleets_pool`, then run
`terraform state mv cyclops_pool.<name> fleets_pool.<name>`. If state records
the old provider address, run `terraform state replace-provider
registry.terraform.io/trycua/cyclops registry.terraform.io/trycua/fleets`
before planning. Back up state and review the plan before applying either
command.

## Choosing a pool sizing mode

New provider releases require exactly one of `replicas` or `autoscaling`.
Existing autoscaled configurations that set both must remove `replicas`:

```terraform
resource "fleets_pool" "linux" {
  # replicas = 0 # remove this line

  autoscaling {
    min_pool_size     = 0
    initial_pool_size = 1
    max_pool_size     = 5
  }
}
```

Static pools keep `replicas` and omit `autoscaling`. In autoscaling mode, do
not configure `replicas`; it reports the current pool target after apply and
refresh. Review the plan before changing modes because switching to static sets
the pool to the configured `replicas` value.
