{/* GENERATED:sandbox-package-facts:start */}
{/* GENERATED:sandbox-package-facts:end */}

## Operating systems and image kinds

The resolver picks the variant each local backend runs (`local=True`, or
`cua sb create`).

| Image input | Guest and kind | Local backend |
| --- | --- | --- |
| `Image.linux()` | `ghcr.io/trycua/linux:24.04` rootfs (amd64, arm64) | Docker or Podman; gVisor when the engine has `runsc`, else `runc` |
| `Image.linux(kind='vm')` | `ghcr.io/trycua/linux:24.04-disk` containerDisk | QEMU (`cua runtime setup qemu` installs it); raises when QEMU is missing |
| `Image.windows()` | `ghcr.io/trycua/windows:2022-disk` containerDisk | Hyper-V on a Windows host that has it, else QEMU |
| `Image.windows('11')` | Windows 11 VM | Local ISO installation |
| `Image.macos()` / `Image.macos('15')` | `ghcr.io/trycua/macos:26` / `:15` Lume VM | Lume on Apple silicon |
| `Image.android()` | Android 14 | Legacy Android emulator adapter |
| `Image.from_registry(ref)` | Any OCI image: rootfs, containerDisk or Lume | Rootfs as a container, containerDisk on QEMU, Lume image on Lume |
| `Image.from_file(...)` | Disk or ISO | QEMU (or Hyper-V) |

Not supported locally: macOS containers and macOS images that are not Lume images. With no
container engine the managed runtime VM is not available yet; install Colima or
Docker Engine. See [Manage local runtimes](/cua-sdk/guides/local-runtimes#check-and-set-up-the-host).

The canonical images are generated from the SDK source:

{/* GENERATED:sandbox-builtin-mappings:start */}
{/* GENERATED:sandbox-builtin-mappings:end */}

The resolver pins them by digest. Override one with
`CUA_IMAGE_LINUX`, `CUA_IMAGE_WINDOWS` or `CUA_IMAGE_MACOS`.

### Windows firmware

A Windows image uses `Firmware.EFI`. `Image.from_registry()` defaults to `os_type='linux'` and does not inspect the
disk, so pass `os_type='windows', kind='vm'` for a Windows containerDisk.

## Image customization

| Operation | Local |
| --- | --- |
| Layers: `apt_install()`, `pip_install()`, `uv_install()`, `run()`, `env()`, `copy()` | Container image: built into the local engine before boot, cached by content. VM image: applied after boot through cua-spacesd (else `Unsupported`) |
| Other layers: `brew_install()`, `choco_install()`, `winget_install()`, `app_install()`, ... | Applied after boot through cua-spacesd (for example `Image.linux()`); `Unsupported` on an image without it |
| `expose()` | Forwarded to a free host port, reported in `sb.exposed_ports` |
| `Image.from_registry(ref, secret=...)` | Authenticated pull |
| `Sandbox.snapshot()` | Not implemented |

`cua image build` and `cua image push` build and publish an image to reuse. See
[Choose and build an image](/cua-sdk/guides/images).

## Workloads, ports and sidecars

| Option | Local |
| --- | --- |
| `command=` | Container entrypoint, or run by cloud-init in a Linux VM |
| `env=` | Container environment or cloud-init |
| `services=`, `wait_for=` | Published loopback ports; probes |
| `sidecars=` | Extra containers sharing the network namespace, reached by name or `localhost`; needs `runtime="runc"` where gVisor would run |
| VM sandboxes with `sidecars=` | Refused |

Sidecars are reached by name on every runtime: the sandbox reaches `db:6379`,
and a sidecar reaches the sandbox at `main`. With sidecars the service names
`main`, `sidecars` and `sc` are reserved.

Readiness is daemon-agnostic: a sandbox is ready when its backend reports it
running, plus the probes you declare (`wait_for=` in Python, `--wait` in the
CLI), and fails fast when the sandbox exits. The computer interfaces use
cua-spacesd on 3211 when the image has it; the SDK reports the `env`
service only for images that do.

### Guest services

| Service | Behavior | Guest requirement |
| --- | --- | --- |
| `env` | Published on port 3211 when the image has cua-spacesd | cua-spacesd, for the computer interfaces and `sb.spacesd()` |
| Names in `services=` | One service per name | Your process listening on that port |
| `port-N` | Added by `.expose(N)` | The application listening on `N` |
| `server` | Added by the older `server_port=` | Your own daemon on that port |

Declaring a service does not install or start it. For an example layout, see the
[Omarchy recipe](/cua-sdk/guides/agent-frameworks).

### Transport availability

| Sandbox | Computer interfaces | `sb.tunnel.forward()` | `sb.service()`, `public_url()`, `mcp()` |
| --- | --- | --- | --- |
| Local (container, QEMU, Lume) | spacesd, else QMP or VNC fallback | Loopback TCP listener | Yes; public URLs through the cua daemon |
| Direct `Sandbox.connect(url=...)` | spacesd | spacesd tunnel | No declared services |
| Legacy adapters (Android ADB, OSWorld, Tart, Hyper-V) | Adapter-specific | ADB forwards ports and Android sockets | Adapter-specific |

For sandboxes in your own cloud, see [Choose where sandboxes run](/cua-sdk/guides/local-runtimes#your-own-cloud)
and [Sandbox interfaces](/cua-sdk/reference/python/interfaces).

## Evidence and limits

The package mappings below are generated from source manifests and image
constants. They describe the clients built from this repository, not the
versions inside a particular deployed image.

{/* GENERATED:sandbox-evidence:start */}
{/* GENERATED:sandbox-evidence:end */}

Verification covers source and offline contract checks, not provisioned
cloud boots, authenticated service requests, or local VM and container boots on every
host. Recipe observations, such as the [Windows Minecraft workflow](/cua-sdk/guides/agent-frameworks),
describe their own tested environment.
