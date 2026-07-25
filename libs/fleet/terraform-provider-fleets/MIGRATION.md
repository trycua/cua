# Migrating from the Cyclops provider

`trycua/fleets` is the public Terraform Registry provider. The Cloud service
APIs, OAuth environment variables, and `OSGymWorkspacePool` CRD remain
unchanged.

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
