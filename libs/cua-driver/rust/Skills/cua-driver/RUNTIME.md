# Runtime, ownership, and authorization

## Preflight and transport

```bash
cua-driver --version
cua-driver status
cua-driver doctor
cua-driver list-tools
cua-driver describe get_window_state
cua-driver describe click
```

Check the resolved executable as well as the version; an installation may have intentional wrappers or multiple products. Preserve them. The client's version alone does not identify an already-running daemon. If the service is mismatched or missing, inspect its existing owner before starting or restarting anything.

Use the advertised `tools/list` contract over MCP. Tool names are `snake_case`; CLI management commands are `kebab-case`. Inspect only unfamiliar tools instead of loading every schema. Descriptions may lag implementation: capability discovery proves advertised shape, while fresh application state proves behavior.

| Client                                     | State ownership                                                                      |
| ------------------------------------------ | ------------------------------------------------------------------------------------ |
| CLI with an explicit non-default `session` | Calls to the same daemon share the named CLI lifecycle until `end_session` or expiry |
| CLI without a named session                | Disposable implicit lease, cleaned up after the response                             |
| Persistent MCP/SDK connection              | One private transport lifecycle; implicit or explicitly named runs live within it    |

Use the CLI by default when a shell is available, or the existing MCP connection when the caller provides one. A multi-call CLI workflow must pass the same nonempty, non-`default` label on every call that accepts it. Tools without a public session parameter, particularly recording controls, need one persistent MCP connection for a coherent lifecycle.

Bare MCP owns a runtime on Windows/Linux and uses the signed service identity on macOS. `cua-driver mcp --socket <endpoint>` explicitly selects a service. One-shot CLI calls are service-backed. Starting a process that serves MCP is not the same as connecting a client: use the harness's MCP integration, not independent shell writes to a dead stdin.

For missing software, use the installation instructions in [README.md](README.md) with user approval. `check-update` is read-only; `update --apply` installs software. After an approved update, check both the executable and daemon. Do not interpret a nonzero updater exit as proof that nothing changed.

`skills update` replaces the installed pack. Back up personal edits before refreshing. `skills install --from main` is for source validation, not proof of compatibility with a release binary. Other platform guides are installed only with `--all-platforms`.

The old computer-use compatibility flag does not add a screenshot tool. Use `get_window_state` or explicitly authorized `get_desktop_state`.

## Session lifecycle

For multi-call work, prefer a short public label and pass the same label on every call that accepts it. Passing it once is not sticky: omission uses the transport's implicit session. Named CLI state is shared only within the daemon's CLI namespace, not with arbitrary MCP connections or a new daemon generation.

`start_session` is optional: actions can establish the run. Use it to configure the initial cursor theme or revive a name after `end_session`. The default idle TTL is five minutes; do not assume a long human permission wait preserves handles.

```bash
cua-driver start_session '{"session":"run-1"}'
# Discover, observe, act, and verify; repeat session:"run-1".
cua-driver end_session '{"session":"run-1"}'
```

Explicit targets own observation/input modality, not session configuration. There is no `deescalate_session`. Do not use retired `capture_scope` settings or legacy escalation-session tools in new workflows. Reserved fields such as `_session_id` belong to the transport, never to an agent.

Keep one controller per shared desktop. Separate sessions/cursors do not isolate global keyboard focus, physical input, a single-instance application, or another observer's snapshot cache. `creates_new_application_instance:true` requests a separate instance where supported; verify distinct processes/windows before concurrent work. For independent MCP work, use independent connections as well.

## Foreground boundary

`delivery_mode:"foreground"` is a user-visible takeover boundary, not an automatic retry. Use it only when the user already authorized visible control for this workflow or after asking. It can change focus, workspace, or the physical cursor; restoration is best-effort and adapter-dependent.

If foreground control is not authorized, stop with the driver's refusal instead of silently escalating. Refresh state and do not retry automatically. An authorized desktop target intentionally uses system input, but it still requires the exact display resource to be admitted. A failed window capture is not desktop permission.

Use Cua's tools rather than shell activation/input shims. `bring_to_front` is reserved for requested persistent foreground state or a focus-proxy surface that must remain active across calls. A native menu operation can also activate temporarily; prefer an in-window control when background posture matters.

## Keep authorization separate from sessions

The trusted host selects one permission profile at startup. `standard` keeps
the normal profile behavior and residual approval requirements, `bounded`
requires a reviewed capability manifest and has no runtime approval path, and
`unrestricted` bypasses Cua approval prompts after explicit risk acceptance.
Hard invariants plus managed and user policy remain binding in every profile.

An optional capability manifest is a deny-by-default ceiling in `standard` and
`unrestricted`; `bounded` requires one. It can remove tools or typed resources
from the selected profile, but it cannot grant a tool, resource, or approval
bypass that another authorization layer denies. Approval is considered only
after the tool and every adapter-attested resource are inside manifest scope.

Use the canonical startup pair together:

```bash
cua-driver mcp \
  --permission-mode standard \
  --capability-manifest ./capabilities.yaml \
  --approve-capability-manifest
```

Capability manifest v3 omits file-level `mode` and `ask.tools`. Its
`allow.tools` list is nonempty. Lifetime fields are optional in `standard` and
`unrestricted`; `bounded` requires both `expires_after` and `idle_timeout`.
The older `--session-policy` names remain compatibility aliases and must not be
used in new configurations.

Starting, ending, naming, reconnecting, or omitting a session never changes
permission authority. A public session label is lifecycle metadata, never a
grant, caller identity, or bearer credential.

## Cursor feedback

Keep the agent overlay visible while controlling pointer or keyboard input. Cursor-bearing and keyboard actions re-show it automatically. The overlay indicates activity, not proof that input landed.

On Linux, ordinary `move_cursor({x,y})` moves only this synthetic cursor. The explicit desktop-pointer escape hatch must not be used unless the user asked for real-pointer control; inspect `describe move_cursor` for that tool's supported target form.

Pixel `click` already animates the cursor. Do not precede it with `move_cursor` at the same point: that adds a second glide. Use `move_cursor` to indicate a location without clicking or seed the cursor before accessibility actions.

The default theme provides session-colored pointer/glow, action marks, and activity animations. Named sessions have stable palette colors. Native hosts own delivery/target badges separately from theme artwork; cursor state is scoped to the private lifecycle.

`set_agent_cursor_enabled` controls idle visibility. `set_agent_cursor_motion` supports `start_handle`, `end_handle`, `arc_size`, `arc_flow`, and `spring` where advertised. Choose preinstalled artwork with `set_agent_cursor_theme`; `cursor_id` on input tools is not a theme selector. Never supply inline theme code or arbitrary source paths through agent tools. The trusted local `cursor-theme` workflow owns validation, compilation, preview, installation, and removal.

Overlay operations need a suitable UI event loop. macOS direct SDK/MCP runtimes without a certified host main-thread adapter can return `facility_unavailable`; do not report that as a successful move. See [EMBEDDING.md](EMBEDDING.md).

## Cleanup and evidence

Finish the owned recording, inspect its result, then end the run. Existing-profile browser cleanup can restore settings Cua enabled; check `end_session` errors and follow [BROWSER.md](BROWSER.md) rather than abandoning cleanup silently.

Do not stop a shared daemon or close the user's app merely because the task ended. Close an application only when requested, using its normal UI and handling unsaved-work prompts without discarding data.

Preserve raw screenshots and action results in a run-specific directory. Report the observed postcondition, relevant version/platform, and any remaining limitation. Review captures for private content before attaching them to a public issue or PR.

For a local MCP HTTP endpoint, the trusted host owns `CUA_DRIVER_RS_MCP_HTTP_PORT` and a host-generated token of at least 32 characters in `CUA_DRIVER_RS_MCP_HTTP_TOKEN`. The client authenticates with the bearer header. Never print credentials or treat a public session label as authentication.
