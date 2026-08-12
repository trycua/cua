# Release procedure

The first public release is `v1.0.0`. Create that tag only after the Cloud PR
is merged to authoritative `main` and Copybara has exported that merge commit to
`trycua/terraform-provider-fleets`.

Before tagging, an organization owner must configure `GPG_PRIVATE_KEY` and
`GPG_FINGERPRINT` in the generated repository and configure the matching public
GPG key with Terraform Registry. The release workflow signs the checksum
artifact. The `Terraform Registry E2E` workflow must pass a clean public
`terraform init` before documentation is updated.
