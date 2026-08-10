# Cua CLI GitHub WIF smoke pool

This stack keeps the `cua-cli-wif-smoke` Fleet pool and namespace available at zero replicas and authorizes GitHub Actions workflows from `trycua/cua` to claim it through GitHub WIF.

Apply it with the manual `Infra: Fleets WIF smoke pool` workflow. The workflow builds `trycua/terraform-provider-fleets` at the pinned merged commit until `trycua/fleets` is published in the Terraform Registry.

The apply job reuses the protected `CUA_CLIENT_ID` and `CUA_CLIENT_SECRET` repository secrets and the canonical Cua OAuth token endpoint.

The pool autoscaler is bounded from zero to one replica. The daily smoke workflow claims a UUID-suffixed sandbox, runs commands through `cua sb exec`, deletes the claim, and suspends the pool back to zero replicas.
