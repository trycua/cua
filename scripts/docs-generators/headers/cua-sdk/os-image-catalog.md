<span id="hosted-fleet-images" />
<span id="ubuntu-2404" />

Any image works as a sandbox; nothing in lifecycle or readiness needs a guest
agent. The `cua-sandbox` computer interfaces (`screen`, `mouse`, `shell`,
`files`) and the Spaces primitives need
[cua-spacesd](/cua-sdk/concepts/how-sandboxes-work) on port 3211. Images without it are
reached through the services you declare. See
[Choose an image](/cua-sdk/guides/images).

Windows images do not include a Windows license. Bring your own license
covering your use, including hosting and virtualization rights. A successful
boot is not a licensing check.

The table is generated from the SDK source (`cua-image` canonical images) and recorded image facts.

{/* GENERATED:sandbox-image-catalog:start */}
{/* GENERATED:sandbox-image-catalog:end */}

## Tiers

Canonical images come in tiers. The tag is `<os-version>[-<tier>][-disk]`:

- **full** (no suffix, the default for `Image.linux()`, `Image.macos()`,
  `Image.windows()` and `cua sb create <os>`): the desktop plus dev tooling.
- **slim** (`-slim`): the minimum that passes `cua-spacesd doctor --strict`
  with every feature. CI and benchmarks use it.
- **xcode** (`-xcode`, `-xcode-<X.Y>`, macOS only): full plus one pinned
  Xcode, its iOS simulator runtime and the Metal toolchain.

Pick one with `Image.linux(tier="slim")` or `cua sb create linux --tier slim`.
A tier that is not published yet raises `ImageNotPublished`; pass its
reference to `Image.from_registry` to use it anyway. Immutable pins append
`-<yyyymmdd>-<sha7>`.

{/* GENERATED:sandbox-image-tiers:start */}
{/* GENERATED:sandbox-image-tiers:end */}

## What's installed

[What's installed](/cua-sdk/reference/image-software) lists the apps and tools
in each image tier, with the versions `cua-spacesd doctor --strict` measured.

## Images in the app pickers

Cua Spaces and the OpenKoalaBots samples offer these images when you create a
Space. The apps and this table read one file, `libs/images/sandbox-images.json`.
Each entry names its distribution (`distro`), so a new Space shows the right
OS icon and System while it is still being created.
Adapter benchmark images join the list once they are published.

{/* GENERATED:sandbox-image-list:start */}
{/* GENERATED:sandbox-image-list:end */}
