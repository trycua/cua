# Workspaces

Workspaces are cua-driver-owned lifecycle and routing boundaries for apps and
windows. They are orthogonal to `capture_scope`: a session may be window- or
desktop-scoped while its launches are routed to a workspace.

Neither `native_space` nor `nested_compositor` is a security sandbox. They do
not isolate files, processes, credentials, or the network. Inspect
`list_workspace_backends` before creating one; its capability flags are the
authoritative contract for the current platform.

## Lifecycle

1. Call `list_workspace_backends`.
2. Call `create_workspace` with `backend: "native_space"` or
   `backend: "nested_compositor"`. `backend: "auto"` selects the platform's
   implemented backend.
3. Either pass the returned `workspace_id` to `launch_app`, or bind it once
   with `start_session({session, workspace_id})` and let later launches inherit
   the binding.
4. Use `move_window_to_workspace` only when the workspace advertises
   `move_existing_window: true`. A nested compositor requires relaunch because
   a Wayland client cannot change display-server connections after launch.
5. Call `close_workspace`. It refuses while sessions are bound unless
   `force: true`; closed records remain as process-local tombstones.

Session end removes only the binding. It does not close a workspace, because
multiple sessions can share one and apps may outlive an idle session.

## Platform behavior

### Linux

`nested_compositor` creates one headless `cua-compositor` child and one private
runtime directory per workspace. `launch_app` applies `XDG_RUNTIME_DIR`,
`WAYLAND_DISPLAY`, and the compositor control socket only to the launched
child; it never changes the daemon's environment. Chromium-family launches get
a workspace-local profile to avoid handing the request to a host single-instance
process. URL/name fallbacks through `xdg-open` are refused for workspace
launches because a desktop broker may route them back to the host compositor.
Closing terminates and reaps recorded direct child processes only after their
`/proc` environment proves they belong to the workspace. This is not cgroup or
process-tree isolation; descendants may need to exit when the compositor
connection closes.

Workspace-scoped capture and input are not advertised yet. Those paths still
read daemon-global Wayland state and must be parameterized before they can be
claimed safely.

### Windows

`native_space` uses only the documented `IVirtualDesktopManager` API. Windows
does not document creating, enumerating, switching, or removing virtual
desktops through this API, so `create_workspace` attaches to a caller-known
desktop GUID supplied as `native_id`. The documented move API is limited to windows
owned by the caller, so this backend does not advertise third-party launch or
move support; attempting a workspace-routed launch fails before starting the
app. Closing releases the driver's reference; it does not delete the desktop.

For Windows 11 24H2 build 26100.2605 or newer, a trusted daemon can set
`CUA_DRIVER_ENABLE_WINDOWS_INTERNAL_DESKTOPS=1` at startup. This enables the
experimental `winvd` shell integration: omitting `native_id` creates a new
driver-owned desktop, `native_id: "current"` attaches to the current desktop,
and third-party app windows can be moved after launch with GUID readback
verification. Closing a driver-owned workspace removes that desktop using a
different desktop as the Windows-required fallback. These shell interfaces are
undocumented and build-sensitive, so capability discovery remains authoritative.
Closing a Windows workspace never terminates its applications; Windows moves
surviving windows from a removed driver-owned desktop onto the fallback desktop.

### macOS

macOS exposes no public API for moving arbitrary third-party windows between
Mission Control Spaces. The experimental `native_space` adapter is therefore
disabled by default. A trusted daemon may opt in at startup with
`CUA_DRIVER_ENABLE_PRIVATE_SPACES=1`, then attach with `native_id: "current"`
or a numeric Space ID. Private SkyLight symbols are resolved dynamically and
every move is verified through a readback; missing symbols, SIP/entitlement
blocking, or a silent no-op returns an error. The adapter intentionally does
not claim Space creation.

## Diorama

Diorama is a future presentation of workspace state, not a workspace backend.
A later `get_workspace_state` can compose the windows, accessibility tree, and
pixels of any backend that advertises capture support without changing this
lifecycle API.
