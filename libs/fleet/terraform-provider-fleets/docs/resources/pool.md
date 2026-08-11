---
page_title: "fleets_pool Resource - Cyclops"
description: |-
  Creates a Cua Fleet computer-use pool.
---

# fleets_pool

Creates an `OSGymSandboxWarmPool`, the `OSGymSandboxTemplate` it references (named `<pool>-template`), and their same-named namespace. Destroy removes all three. Import IDs are pool names.

## Static pool

```terraform
resource "fleets_pool" "linux_static" {
  name                 = "training-linux-static"
  replicas             = 3
  cpu_cores            = 4
  memory               = "8Gi"
  container_disk_image = "296062593712.dkr.ecr.us-west-2.amazonaws.com/osgym-workspace:latest"
}
```

## Autoscaled pool

```terraform
resource "fleets_pool" "linux_autoscaled" {
  name                 = "training-linux-autoscaled"
  cpu_cores            = 4
  memory               = "8Gi"
  container_disk_image = "296062593712.dkr.ecr.us-west-2.amazonaws.com/osgym-workspace:latest"

  autoscaling {
    min_pool_size     = 0
    initial_pool_size = 3
    max_pool_size     = 20
  }
}
```

## Arguments

- `name` - Pool and namespace DNS label. Changing it replaces the resource.
- `replicas` - Static desired warm pool size. Exactly one of `replicas` or `autoscaling` must be configured. In autoscaling mode, do not configure it; after apply and refresh it reports the current pool target.
- `cpu_cores` - Virtual CPUs per sandbox.
- `memory` - Kubernetes memory quantity per sandbox.
- `container_disk_image` - OCI containerDisk or runtime image.
- `image_pull_secret` - Image pull secret; defaults to `ecr-credentials`.
- `runtime` - `kubevirt`, `macos`, or `gvisor`; defaults to `kubevirt`.
- `firmware` - `bios` or `efi`; defaults to `bios`.
- `readiness_probe_json` / `liveness_probe_json` - Kubernetes probe objects encoded as JSON.
- `service` - Repeatable service with `name`, `target_port`, and optional `protocol`.
- `autoscaling` - Claim-driven autoscaling limits. Exactly one of `replicas` or `autoscaling` is required.
- `autoscaling.min_pool_size` - Minimum warm pool size while autoscaling is enabled.
- `autoscaling.initial_pool_size` - Initial pool target when autoscaling starts.
- `autoscaling.max_pool_size` - Maximum autoscaled pool size; defaults to `50` when omitted.

## Read-only Attributes

`namespace` and `template_name` identify the objects Fleet created. In autoscaling mode, `replicas` reports the current pool target so scaling changes remain visible in Terraform state. `current_replicas` and `ready_replicas` report the number of current and ready sandboxes.

## Import

```shell
terraform import fleets_pool.linux training-linux
```
