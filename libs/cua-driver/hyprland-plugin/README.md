# Cua Hyprland plugin

This directory is the source of truth for an optional, experimental Hyprland
plugin. The plugin is disabled by default and is not part of the portable Cua
Driver installation. It must be built for the exact Hyprland version that will
load it; a plugin built against another Hyprland ABI must fail closed rather
than attempt to run. Load checks compare Hyprland's full client/server ABI
fingerprint, including the compositor commit and linked Hyprland library
versions, before calling other compositor APIs. The package and validation
record must also match the compiler toolchain because Hyprland passes C++
objects across the plugin API.

The module must dynamically link the C++ runtime used by the compositor. The
build checks its ELF dependencies and rejects a static C++ runtime; a compiler
copied without its runtime libraries can otherwise silently select a static
archive. Do not load a module from a failed build.

The current foundation is discovery-only. It advertises protocol status and
liveness, but it rejects every input mutation with the typed
`background_unavailable` result. Target-addressed mutation remains gated on
successful second-seat spikes and acceptance of the corresponding RFC.
Neither stable Cua Driver `0.23.2` nor nightly provides isolated background
input through this plugin. Here, discovery means negotiation, status, and
liveness; it does not mean application target discovery or input delivery.

This design builds on Dillon DuPont's Hyprland injection prototype. That credit
records the prototype lineage; it does not claim that the prototype or input
mutation support has shipped in Cua Driver.

## Build

Install the Hyprland headers for the exact installed compositor version, plus
CMake 3.30 or newer, Ninja, and the same compiler family and version used to
build Hyprland. The plugin and its libraries use Hyprland's C++26 language
mode. From this directory:

```bash
cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release
cmake --build build
```

For a pinned acceptance build, also set
`-DCUA_HYPRLAND_EXPECTED_VERSION=0.56.2` (or the exact intended pkg-config
version). The build refuses a different header version. The status-command
adapter supports both Hyprland `0.56.2`'s `SHyprCtlCommand` API and the newer
Socket1 API. CMake compile-checks the installed `PluginAPI.hpp` and selects a
declared tier; an unknown command API fails configuration. The probes use
C++26 and repeat on reconfiguration so a header upgrade cannot reuse a cached
tier. Both adapter paths run in the local mock suite. A separate configuration
test checks tier selection, compiler flags, and unsupported headers. Real-header
and live-load evidence remain required on Linux.

On a non-Linux review host, build only the portable protocol, transport shim,
and mocked plugin-API tests:

```bash
cmake -S . -B build -DCUA_HYPRLAND_BUILD_PLUGIN=OFF -DBUILD_TESTING=ON
cmake --build build
ctest --test-dir build --output-on-failure
```

That workflow is not Linux or Hyprland evidence. Release validation must use
the production `SOCK_SEQPACKET`/`SO_PEERCRED` path and real matching Hyprland
headers.

Do not retain or redistribute the resulting module across a Hyprland upgrade.
Rebuild it against the upgraded Hyprland package instead. The local Arch recipe
in `packaging/arch/` installs a version-pinned development package without
changing Hyprland configuration.

## Load, inspect, and unload

Loading is always an explicit operator action. For a local build, substitute
the actual module path emitted by CMake:

```bash
hyprctl plugin load "$PWD/build/cua-hyprland-plugin.so"
hyprctl -j cua:status
hyprctl plugin unload "$PWD/build/cua-hyprland-plugin.so"
```

For the Arch development package, use:

```bash
hyprctl plugin load /usr/lib/cua/hyprland/cua-hyprland-plugin.so
hyprctl -j cua:status
hyprctl plugin unload /usr/lib/cua/hyprland/cua-hyprland-plugin.so
```

Do not add a `plugin =` line to Hyprland configuration until the ABI match and
status output have been checked in a disposable session. A mismatched module
may be refused by Hyprland or fail during load. If load fails after a Hyprland
upgrade, leave it unloaded, rebuild against the new exact package, and retry.
The portable driver remains available without this plugin.

`hyprctl -j cua:status` is the authoritative runtime status surface. Consumers
must check its compositor epoch, protocol version, advertised capabilities,
socket path, discovery-only state, and ABI identity before connecting. A loaded
module alone is not evidence that target discovery or mutation is available.

The local transport is also disabled by default. To exercise negotiation in a
disposable session, set the following and reload the configuration after the
plugin loads. For Omarchy's Lua configuration, add:

```lua
hl.config({plugin = {cua = {enabled = true}}})
```

For a legacy Hyprland configuration, use:

```text
plugin:cua:enabled = true
```

Run `hyprctl reload` after editing the configuration. The plugin reconciles
transport state on config reload; a runtime `hyprctl eval` or `keyword` alone
does not start or stop it. To disable the transport, remove the setting or set
it to false in the configuration and reload. Unloading the plugin also closes
the socket and every connection immediately.

This enables same-user status and liveness packets only. It does not enable
pointer, keyboard, scroll, drag, target discovery, or another input route.

## Security boundary

- The socket lives at
  `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/cua-inject-v2.sock` for
  the current Hyprland instance, not in a global temporary directory, and is
  created with mode `0600`.
- The server verifies `SO_PEERCRED` and accepts only peers with the compositor
  user's UID. This authenticates only the local Unix user, not a particular Cua
  process. Filesystem permissions are defense in depth, not authorization.
- Same-UID discovery is acceptable for this mutation-free foundation. Mutation
  capabilities stay disabled until the RFC defines an operator authorization,
  credential or lease lifetime, revocation, and replay-resistant binding to the
  negotiated compositor epoch.
- The transport is bounded, nonblocking Unix `SOCK_SEQPACKET`; oversized,
  truncated, malformed, or unsupported packets are rejected without mutation.
- A client must complete `HELLO` within five seconds. A negotiated client must
  send traffic such as `PING` at least once per minute or the plugin closes the
  idle connection; long-lived driver processes must reconnect and renegotiate.
- Future target handles must be opaque tokens scoped to one compositor epoch.
  Clients must discard them when the epoch changes and must not infer window
  identity from token contents. The discovery-only build issues no target
  handles.
- Protocol capabilities are negotiated explicitly. Absence of a capability is
  a refusal, never permission to try a weaker route.
- The plugin never redirects a target-addressed request to the primary seat,
  focused window, pointer location, or another target.
- A transport acknowledgement proves only receipt and validation. Application
  behavior requires a separate typed result and independent state evidence.

The wire contract is documented in `protocol/cua-inject-v2.md`.

## Relationship to current Hyprland support

Existing Cua Driver paths for capture, target identity, foreground input, and
AT-SPI semantic actions are separate from this plugin. A semantic action can
sometimes update a background application's accessibility object without raw
pointer or keyboard synthesis; that does not prove isolated synthetic input.

Validate stock Cua Driver `0.23.2` desktop capture, window enumeration,
foreground actions, and AT-SPI actions separately. Available protocols and
accessibility services establish prerequisites; successful actions require
application-state evidence. The host Hyprland/plugin ABI pair, plugin transport,
opaque target binding, and isolated raw background input each require their
own acceptance evidence on the exact host and candidate module.

Compositor administration also remains explicit. Integrations may invoke
`hyprctl` for an operator-requested workspace, monitor, window-rule, focus, or
DPMS operation, but a Cua background action must never hide a target-delivery
failure by changing compositor state. DPMS-on is a precondition for visual
delivery evidence; an all-black frame while DPMS is off is not success.

`hyprctl` is the compositor administration and plugin status boundary. The
plugin adds `hyprctl -j cua:status`, but it does not turn arbitrary `hyprctl`
commands into application-targeted input or application-delivery proof. Tests
must observe whether the stock foreground route explicitly activated a window
and must keep that behavior separate from plugin mutation, which remains
disabled.

Application enumeration and call authorization also stay in Cua Driver.
Filtering `list_apps`, choosing a one-shot `cua-driver call` versus a long-lived
`cua-driver serve` process, and enforcing per-call capability or policy rules
are not plugin capabilities. The plugin authenticates and negotiates its local
protocol; it does not replace the driver's tool policy or process lifecycle.

## Acceptance path

Promotion beyond discovery requires focused protocol and security tests, Fleet
image packaging and lifecycle tests, and the repository's representative Linux
catalogs. The final gate is a physical Omarchy machine running the exact
candidate Hyprland/plugin pair, with focus, z-order, cursor, input-isolation,
and fixture-state evidence. VM or Fleet evidence supplements but does not
replace that physical gate, which must run after the Fleet lane passes.

The exact initial host acceptance baseline is:

| Component                   | Required baseline |
| --------------------------- | ----------------- |
| Omarchy                     | `4.0.2-1`         |
| Hyprland                    | `0.56.2`          |
| xdg-desktop-portal-hyprland | `1.4.1`           |
| Cua Driver                  | `0.23.2`          |
| Session and display manager | UWSM / SDDM       |

This baseline is an acceptance target, not a statement that the reported host
or its Hyprland/plugin ABI pair has passed validation. It is also not a claim
that isolated background input ships in stable `0.23.2` or nightly. The plugin
remains discovery-only. The final physical run has explicit foreground
activation observation, GTK3, GTK4, Qt6, LibreOffice, Chromium/Ozone, Electron,
DPMS-off refusal, and one-shot and long-lived graphical-session client rows;
see `tests/README.md` for the required evidence.
