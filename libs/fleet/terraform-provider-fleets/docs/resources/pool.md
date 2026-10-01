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
  container_disk_image = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-e5d853a9"
}
```

## Autoscaled pool

```terraform
resource "fleets_pool" "linux_autoscaled" {
  name                 = "training-linux-autoscaled"
  cpu_cores            = 4
  memory               = "8Gi"
  container_disk_image = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-e5d853a9"

  autoscaling {
    min_pool_size     = 0
    initial_pool_size = 3
    max_pool_size     = 20
  }
}
```

## Public image without registry credentials

Omit `image_pull_secret` to omit `imagePullSecret` from the Fleet template and
let Kubernetes pull an anonymously accessible image.

```terraform
resource "fleets_pool" "public_gvisor" {
  name                 = "public-gvisor"
  cpu_cores            = 4
  memory               = "8Gi"
  container_disk_image = "ghcr.io/example/public-image@sha256:..."
  runtime              = "gvisor"

  autoscaling {
    min_pool_size     = 0
    initial_pool_size = 0
    max_pool_size     = 20
  }
}
```

## SDK-equivalent pool

`cua fleet pool export --terraform` prints this shape for a pool made with
`Sandbox.create` or `Pool.apply`: a process, claim-scoped secrets, a warm
floor of one, and lifecycle TTLs.

```terraform
resource "fleets_pool" "agent" {
  name                      = "agent-linux"
  cpu_cores                 = 4
  memory                    = "8Gi"
  container_disk_image      = "ghcr.io/example/agent@sha256:..."
  runtime                   = "gvisor"
  command                   = ["python", "-m", "http.server", "8765"]
  claim_secrets             = true
  idle_ttl_seconds          = 86400
  ttl_policy                = "Cascade"
  ttl_seconds_after_created = 604800

  autoscaling {
    min_pool_size     = 1
    initial_pool_size = 1
    max_pool_size     = 20
  }

  service {
    name        = "http"
    target_port = 8765
  }
}
```

## Arguments

- `name` - Pool and namespace DNS label. Changing it replaces the resource.
- `replicas` - Static desired warm pool size. Exactly one of `replicas` or `autoscaling` must be configured. In autoscaling mode, do not configure it; after apply and refresh it reports the current pool target.
- `cpu_cores` - Virtual CPUs per sandbox.
- `memory` - Kubernetes memory quantity per sandbox.
- `container_disk_image` - OCI containerDisk or runtime image.
- `image_pull_secret` - Optional image pull secret. Omit it for an anonymous public-image pull.
- `runtime` - `kubevirt`, `macos`, or `gvisor`; defaults to `kubevirt`.
- `firmware` - `bios` or `efi`; defaults to `bios`.
- `readiness_probe_json` / `liveness_probe_json` - Kubernetes probe objects encoded as JSON.
- `command` - Entrypoint command list (replaces the image entrypoint). Pod runtimes (`gvisor`, `macos`) run it; KubeVirt ignores it.
- `claim_secrets` - Opt in to claim-scoped secret delivery at `/run/cua` (the per-claim env token and a claim's `secretRef`). Claims with a `secretRef` fail on pools without it.
- `idle_ttl_seconds` - Delete the pool after this many seconds with no Pending or Bound claims. Omit it to never reap for idleness.
- `ttl_policy` - What a TTL expiry deletes: `Retain` (the pool only, the behavior when omitted) or `Cascade` (also the pool's dead unbound claims; never Bound claims, the namespace or volumes).
- `ttl_seconds_after_created` - Delete the pool this many seconds after it was created. Omit it to never reap by age.
- `service` - Repeatable service with `name`, `target_port`, and optional `protocol`.
- `autoscaling` - Claim-driven autoscaling limits. Exactly one of `replicas` or `autoscaling` is required.
- `autoscaling.min_pool_size` - Minimum warm pool size while autoscaling is enabled.
- `autoscaling.initial_pool_size` - Initial pool target when autoscaling starts.
- `autoscaling.max_pool_size` - Maximum autoscaled pool size; defaults to `50` when omitted.

Removing `command`, `claim_secrets`, a probe or a lifecycle attribute from the
configuration clears it from the Fleet object. A pool TTL that expires deletes
the pool; the next apply creates it again.

## Read-only Attributes

`namespace` and `template_name` identify the objects Fleet created. In autoscaling mode, `replicas` reports the current pool target so scaling changes remain visible in Terraform state. `current_replicas` and `ready_replicas` report the number of current and ready sandboxes.

## Import

```shell
terraform import fleets_pool.linux training-linux
```
