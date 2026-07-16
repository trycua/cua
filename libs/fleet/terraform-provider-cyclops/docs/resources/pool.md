---
page_title: "cyclops_pool Resource - Cyclops"
description: |-
  Creates a Cyclops computer-use pool.
---

# cyclops_pool

Creates an `OSGymWorkspacePool` and its same-named namespace. Destroy removes both. Import IDs are pool names.

```terraform
resource "cyclops_pool" "linux" {
  name                 = "training-linux"
  replicas             = 3
  cpu_cores            = 4
  memory               = "8Gi"
  container_disk_image = "296062593712.dkr.ecr.us-west-2.amazonaws.com/osgym-workspace:latest"

  autoscaling {
    min_pool_size     = 0
    initial_pool_size = 3
    max_pool_size     = 20
  }

  service {
    name        = "ssh"
    target_port = 22
    protocol    = "TCP"
  }
}
```

## Arguments

- `name` - Pool and namespace DNS label. Changing it replaces the resource.
- `replicas` - Desired warm pool size.
- `cpu_cores` - Virtual CPUs per sandbox.
- `memory` - Kubernetes memory quantity per sandbox.
- `container_disk_image` - OCI containerDisk or runtime image.
- `image_pull_secret` - Image pull secret; defaults to `ecr-credentials`.
- `runtime` - `kubevirt`, `macos`, or `gvisor`; defaults to `kubevirt`.
- `firmware` - `bios` or `efi`; defaults to `bios`.
- `readiness_probe_json` / `liveness_probe_json` - Kubernetes probe objects encoded as JSON.
- `service` - Repeatable service with `name`, `target_port`, and optional `protocol`.
- `autoscaling` - Optional claim-driven autoscaling limits.

## Read-only Attributes

`namespace`, `phase`, `total_count`, `available_count`, and `claimed_count` are populated from Cyclops.

## Import

```shell
terraform import cyclops_pool.linux training-linux
```
