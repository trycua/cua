# Cua CLI GitHub WIF smoke pool

This stack keeps the `cua-cli-wif-smoke` Fleet pool and namespace available at zero replicas and authorizes GitHub Actions workflows from `trycua/cua` to claim it through GitHub WIF.

Apply it with the manual `Infra: Fleets WIF smoke pool` workflow. The workflow builds `trycua/terraform-provider-fleets` at the pinned merged commit until `trycua/fleets` is published in the Terraform Registry.

The apply job uses a dedicated user-owned Fleets key stored as protected repository secrets:

- `FLEETS_TERRAFORM_CLIENT_ID`
- `FLEETS_TERRAFORM_CLIENT_SECRET`
- `FLEETS_TERRAFORM_TOKEN_URL`

The pool starts at zero replicas. The daily smoke workflow scales it to one, claims a UUID-suffixed sandbox, runs commands through `cua sb exec`, deletes the claim, and suspends the pool back to zero replicas.

If the pool was previously created with different credentials, manually dispatch the apply workflow once with `recreate_pool` enabled. The recovery deletes only the managed pool with the original bootstrap credentials, then recreates the pool and trust policy under the dedicated owner key.
