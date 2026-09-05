# Hyprland plugin test plan

The plugin is optional, disabled by default, and discovery-only until the
second-seat design is proven and its RFC is accepted. Tests must fail closed;
they must not enable mutation merely to exercise a path.

Neither stable Cua Driver `0.23.2` nor nightly provides isolated background
input through this plugin. Discovery currently covers protocol negotiation,
status, and liveness only; application target discovery and input mutation are
not enabled.

Record stock `0.23.2` desktop capture, window enumeration, foreground actions,
and AT-SPI actions separately. Available Wayland protocols and AT-SPI services
establish prerequisites only. Plugin ABI compatibility and isolated raw
background delivery require their own evidence. Host Hyprland validation
requires the exact versions and rows below.

## Focused pre-merge checks

Run against the exact Hyprland headers used for the build:

The portable protocol tests run on any host with C++26 support. The Unix transport smoke
uses `SOCK_STREAM` only on non-Linux developer hosts that lack AF_UNIX
`SOCK_SEQPACKET`; release evidence must run the same test on Linux, where the
production `SOCK_SEQPACKET` and `SO_PEERCRED` paths are compiled and exercised.
Use `-DCUA_HYPRLAND_BUILD_PLUGIN=OFF` when configuring those tests on a
non-Linux host. Passing the shim is not Linux transport evidence.

1. Build the module with warnings treated as errors where the toolchain allows.
   Compile both the Hyprland `0.56.2` legacy status-command adapter and the
   newer Socket1 adapter in the mock suite. The CMake probe test also verifies
   C++26, pkg-config flag propagation, unsupported-header rejection, and tier
   changes within one build directory. Record the compositor and plugin
   compiler family and version; they must match for live load evidence.
   The ELF dependency check must accept a shared C++ runtime and reject a
   statically linked runtime. Preserve both outputs if an incomplete compiler
   staging directory causes that negative case.
2. Exercise binary packet parsing at zero, boundary, and over-limit sizes,
   including bad magic, unsupported versions, nonzero flags, truncation, queue
   saturation, and disconnect during nonblocking reads and writes.
3. Verify socket placement in the current Hyprland per-instance runtime
   directory, inode mode `0600`, safe stale-socket handling, and cleanup that
   cannot remove another process's file.
4. Verify `SO_PEERCRED` accepts the compositor UID and refuses another UID or
   unavailable credentials before processing requests.
5. Force an I/O-thread poll failure and verify the listener closes, the socket
   path disappears, future connects fail, and status retains the failure.
6. Exercise `EMFILE`/`ENFILE` accept failures under a bounded file-descriptor
   limit and verify retries are time-bounded rather than a readable-listener
   busy loop.
7. Verify protocol, epoch, and capability negotiation, including stale epochs,
   unsupported versions, absent capabilities, handshake and post-handshake idle
   timeouts, keepalive traffic, reconnects, and compositor restarts.
8. Verify opaque target tokens cannot be correlated with pointers, object IDs,
   or PIDs; are pruned on destruction; and never survive an epoch change.
9. Verify every mutation shape returns exact `background_unavailable` without
   focus, z-order, cursor, target state, primary-seat activity, or
   unrelated-window changes. Error detail is diagnostic and is not a stable
   machine-readable reason field in protocol v2.
10. Verify a transport `ack` cannot satisfy an application-delivery assertion;
    the harness must require a typed result and an independent state oracle.

An ABI-negative test builds against one Hyprland version and attempts to load
under a different version in a disposable environment. The acceptable outcome
is an explicit refusal or unavailable status with no socket. A crash, partial
registration, or capability advertisement is a failure.

With the exact module already loaded and discovery enabled in a disposable
Hyprland session, run the live transport check:

```bash
python3 tests/live_discovery.py --reload-module /absolute/path/to/cua-hyprland-plugin.so
```

The script checks real packet negotiation, liveness, all six mutation refusals,
socket mode, unchanged compositor window/workspace/cursor state, connection
closure on unload, and a fresh epoch on reload. It does not validate application
state, a live foreground grab, hardware input isolation, or a different-UID
peer. Those remain separate acceptance rows. The reload option unloads and
reloads the named module; omit it for a transport-only check.

To test two-instance isolation and clean restart inside a disposable Hyprland
Wayland desktop, run the owned-process lifecycle runner:

```bash
python3 tests/nested_lifecycle.py --module /absolute/path/to/cua-hyprland-plugin.so
```

The runner starts two nested Hyprland processes with temporary configs, finds
each by its exact child PID, and loads the matching module only into those
processes. It runs the live refusal check in both, restarts one, and checks old
connection closure, socket removal, fresh socket/epoch, and sibling liveness.
It stops both owned processes on success or failure and preserves temporary
logs. Forced termination is a failure, not a clean-restart pass. The parent
must still answer version queries afterward; its config is never edited and
the module is never loaded into it. Use a disposable parent because nested
windows can affect its focus and compositor startup can update session state.

Record the source SHA, module digest, environment, stdout, stderr, and retained
logs. The JSON result distinguishes this check from application delivery and
physical input isolation. It does not certify package installation, direct DRM
startup, arbitrary ABI combinations, or the complete driver desktop matrix.
Hosted CI tests the runner's cleanup/error paths with mocks; native results
require running it in the Fleet guest or another declared Wayland environment.

## Fleet packaging lane

Build an Omarchy/Arch Fleet image from the monorepo source using
`packaging/arch/PKGBUILD`. Record the source SHA, exact Hyprland package version,
plugin package version, module digest, and image identity. The lane must prove:

- installation only under `/usr/lib/cua/hyprland/`, with no configuration edit,
  service enablement, autoload, or replacement of Hyprland-owned files;
- explicit load, JSON discovery/status inspection, one-shot client negotiation,
  long-lived client reconnect, explicit unload, and clean Hyprland restart;
- exact ABI dependency failure after a simulated Hyprland package change;
- socket isolation between two Hyprland instances for the same user and refusal
  of a different-UID peer; and
- all discovery-only mutation refusals under fixture-state, focus, z-order,
  cursor, and input-leak oracles.

The Fleet lane must also identify whether each client is a one-shot process or
a long-lived process created inside the graphical session. A process launched
outside that session and given copied display variables is not equivalent
evidence. Chromium and Electron rows must record their effective Ozone backend
and confirm they render and expose the intended fixture before testing a
refusal.

Run the repository's ordinary Linux source/unit checks and relevant Cua Driver
catalogs on the exact candidate SHA. Fleet validates reproducible packaging and
lifecycle behavior; it is not the final hardware claim.

## Physical Omarchy final gate

After the Fleet packaging lane passes, and before any capability or release
claim, repeat the candidate package on a bare-metal Omarchy machine using the
exact Hyprland/plugin pair. Record status before load, after load, after
discovery, after refused mutations, after unload, and after a compositor
restart. Independently verify application state, focus, z-order, cursor
position, and absence of primary-seat or unrelated-window input.

The physical run is the final acceptance gate. VM, nested compositor, or Fleet
success cannot replace it. If the candidate SHA, Hyprland package, protocol,
packaging, mutation behavior, or harness changes afterward, rerun the evidence
affected by that change before promotion.

The exact initial baseline is Omarchy `4.0.2-1`, Hyprland `0.56.2`,
xdg-desktop-portal-hyprland `1.4.1`, Cua Driver `0.23.2`, and UWSM/SDDM. Record
all five values with the candidate SHA and plugin module digest.

Run each row separately and retain the typed protocol result plus the listed
independent oracle. All application rows require DPMS on. Because the plugin is
discovery-only, mutation attempts must return `background_unavailable` and
must leave the application, focus, z-order, cursor, primary seat, and unrelated
windows unchanged.

| Validation row                      | Required evidence                                                                                                                                                                                                             |
| ----------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Stock `0.23.2` capture              | A non-black frame with fixture-owned visual markers; classify this as stock capture evidence, not plugin evidence.                                                                                                            |
| Stock `0.23.2` foreground action    | Before/after active-window identity proving whether the driver explicitly activated the target, plus fixture state; activation is foreground-route behavior, not isolated background input.                                   |
| Stock `0.23.2` AT-SPI action        | Accessibility and fixture-state evidence identifying the semantic route; no raw-input claim.                                                                                                                                  |
| GTK3 native Wayland                 | Typed refusal and unchanged GTK3 fixture state.                                                                                                                                                                               |
| GTK4 native Wayland                 | Typed refusal and unchanged GTK4 fixture state.                                                                                                                                                                               |
| Qt6 native Wayland                  | Typed refusal and unchanged Qt6 fixture state.                                                                                                                                                                                |
| LibreOffice native Wayland          | Typed refusal and unchanged document state.                                                                                                                                                                                   |
| Chromium with Ozone/Wayland         | Record Chromium version, launch flags, and effective Wayland Ozone backend; prove the fixture renders, then require typed refusal and unchanged page state.                                                                   |
| Electron with Ozone/Wayland         | Record Electron/Chromium versions, launch flags, and effective Wayland Ozone backend; prove the fixture renders, then require typed refusal and unchanged application state.                                                  |
| DPMS off                            | Require typed refusal before dispatch with no action claimed; do not turn DPMS on implicitly, and never treat a black capture as success.                                                                                     |
| One-shot graphical-session client   | Launch one fresh process from the real graphical-session environment, complete discovery negotiation, receive typed mutation refusal, and exit cleanly without relying on state from another invocation.                      |
| Long-lived graphical-session client | Launch inside the real graphical session, negotiate discovery, survive repeated calls, detect compositor/plugin restart and the stale connection, reconnect and negotiate the new epoch, then receive typed mutation refusal. |

The bare-metal result is the final certification after Fleet. A missing row,
an untyped failure, a black frame treated as capture success, focus or cursor
movement, input leakage, or a client that cannot reconnect fails certification.

`hyprctl` evidence is limited to compositor administration and the plugin's
`cua:status` surface. Record every focus, workspace, DPMS, load, status, and
unload command separately; none is application mutation proof. Likewise,
`list_apps` filtering and the choice and authorization of `cua-driver serve` or
individual `cua-driver call` requests are driver-side tests, not plugin
capabilities. They may be exercised to diagnose an agent integration, but
must not appear in the plugin capability result.
