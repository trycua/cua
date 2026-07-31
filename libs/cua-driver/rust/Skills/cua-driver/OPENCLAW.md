# OpenClaw managed-provider profile

Profile id: `openclaw-mcp`

Use this profile only when OpenClaw has already selected and bound a Cua Driver
provider for the current execution. OpenClaw owns the runtime, transport,
permission mode, native machine, and trusted resource bindings. The model uses
only the live provider-native tools and schemas supplied for that binding.

## Hard boundaries

- Never run the `cua-driver` CLI or a shell command to reach Cua during this
  execution.
- Never start, stop, install, update, or reconfigure Cua Driver.
- Never choose a socket, transport, machine, provider, native session handle,
  permission mode, or helper executable.
- Never treat the public session label as authority. It is only a display and
  lifecycle label.
- Never invent a tool, field, action route, or fallback that is absent from the
  live provider schema.
- A missing or incompatible profile is a provider compatibility failure. Do not
  fall back to generic computer-use guidance.

## Session lifecycle

OpenClaw binds one trusted Cua session to the current execution lease. If
`start_session` is present, call it once with the public session label supplied
by the host and the requested capture scope. Pass that same label on subsequent
calls that accept `session`. End with `end_session` when it is present.

Do not reuse browser targets, tabs, page refs, element indices, coordinates, or
recording resources across execution leases.

## Snapshot, action, verification

Every state-changing action uses this loop:

1. Observe the current target with `get_window_state` in window scope or
   `get_desktop_state` in desktop scope.
2. Resolve a fresh `element_index`, browser ref, or target-local pixel point.
3. Invoke exactly one action through the bound provider tool.
4. Observe again and verify the intended effect.

Treat a successful transport response as delivery evidence, not proof of
effect. Prefer structured `effect`, `verified`, `degraded`, `refusal`, and
`escalation` fields when the live result exposes them.

## Action ladder

Use the least disruptive route that can satisfy the task:

1. background accessibility action with a fresh `element_index`;
2. background target-local pixel action when accessibility cannot address the
   control;
3. exact browser-page action when a live browser target, tab, and ref exist;
4. foreground delivery by setting `delivery_mode:"foreground"` only after the
   background routes report a structured refusal or fail verification; and
5. explicit desktop escalation through `escalate_session` only after the
   window ladder is exhausted and the live schema permits it.

`bring_to_front` is a separate platform operation. It is not the ordinary
foreground-delivery rung and must not replace `delivery_mode:"foreground"`.

Refresh state after any popup, navigation, focus change, window replacement, or
failed verification. Do not ask the model to implement a deterministic retry
state machine. The provider and Gateway own routing policy; the model chooses
among only the live, authorized actions.

## Resource bindings

In a managed provider, recording roots, download roots, upload files, replay
artifacts, and native helpers are selected by trusted host code. Use only the
opaque resource identifiers or path-free fields present in the live schema.
Never send a native path or try to discover one. Keep observation screenshots
inline and do not request click debug-image output.

Read the matching `OPENCLAW_MACOS.md`, `OPENCLAW_WINDOWS.md`, or
`OPENCLAW_LINUX.md`. Read `OPENCLAW_BROWSER.md` for browser work and
`OPENCLAW_RECORDING.md` only when the provider exposes those capabilities.
