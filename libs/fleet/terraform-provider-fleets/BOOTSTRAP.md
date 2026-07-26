# Generated repository bootstrap

`trycua/terraform-provider-fleets` is intentionally empty before the first
export. After the Cloud PR is merged, install the existing Copybara GitHub App
with `contents: write`, protect `main`, and allow only that App to bypass branch
protection. Do not add human-maintained source files.

Dispatch `Copybara Provider Export` with `bootstrap` enabled. The workflow uses
`--force --squash`, which creates the empty destination's `main` branch and
writes the flattened `cyclops-cs/terraform-provider-fleets` projection. After
bootstrap, all destination changes come from Cloud through
`CloudTerraformProviderFleets-RevId`; no direct writers are permitted.
