# cua-teleport

Teleport **send**: move a desktop app session (open tabs, profile, sign-in)
from the user's machine into a sandbox. It runs next to the app, never inside a
sandbox.

| Half | Crate | Where it runs |
|------|-------|---------------|
| Send (this crate) | `libs/cua/crates/cua-teleport` | the user's machine (SDK, `cua` CLI, apps) |
| Receive | `libs/cua-spacesd/crates/cua-spacesd-teleport` | inside the sandbox, in cua-spacesd |
| Shared contract | `libs/cua/crates/cua-teleport-bundle` | both; effect-free (format, types, per-app layouts) |

The halves never link each other. They meet on the wire at
`cua.env.v1.TeleportService`:

1. `GetManifest` asks the guest whether it can import the app (the sender
   checks this before prompting anyone).
2. `ImportSession` uploads the `SessionBundle` in unary, offset-addressed
   chunks (so gRPC-Web works). The last chunk commits with the SHA-256 of the
   whole bundle, and the guest verifies, imports and optionally launches.

## Flow

```
manifest ─▶ selection ─▶ Approval (consent) ─▶ OS authorization (sensitive items only)
        ─▶ capture (SessionBundle) ─▶ chunked upload ─▶ guest import + launch
```

Nothing is read before consent and OS authorization pass. A declined
`Approval` returns `Error::NotApproved`. A denied Touch ID or passcode prompt
returns an `Error::Teleport` that says the export was not authorized.

## Rust API

```rust
use std::sync::Arc;
use cua_teleport::{AppRef, AutoApprove, Platform, Selection, Teleporter, TransferScope};

let env: cua_spacesd_client::SpacesdClient = /* e.g. sandbox.spacesd().inner().clone() */;
let app = AppRef {
    app_id: "com.google.Chrome".into(), // or "Slack", "claude-code", …
    display_name: "Google Chrome".into(),
    platform: Platform::current(),
};

// One call: built-in providers on the real host.
let outcome = cua_teleport::send(
    &env,
    &app,
    TransferScope::FullProfile,
    Selection::Default,        // or ::All, or ::Items(vec![rel_path, …])
    Arc::new(AutoApprove),     // or any Fn(&ApprovalRequest) -> bool
)
.await?;
println!("{} bytes, launched: {}", outcome.bundle_bytes, outcome.launched);
```

More control comes from `Teleporter`:

```rust
let t = Teleporter::new()                         // or ::with_host(fake), ::with_registry(r)
    .options(cua_teleport::SendOptions {
        launch_after: true,
        chunk_bytes: None,                        // default: the guest's preferred size (1 MiB)
        progress: Some(Arc::new(|sent, total| eprintln!("{sent}/{total}"))),
        ..Default::default()
    });
let manifest = t.manifest(&app, TransferScope::FullProfile)?;   // blocking; never prompts
let outcome = t.send(&env, &app, TransferScope::FullProfile, Selection::Default, approval).await?;
```

- `ExportRegistry::with_builtin_host_and_chrome_profile(host, Some("Profile 1".into()))`
  picks a Chrome profile by name or path.
- `ExportRegistry::infos()` lists providers (id, app ids, platforms, install
  probe) for consent UIs. `cua_teleport::is_installed(&probe)` evaluates a
  probe on this host.
- `Teleporter::export(...)` captures without uploading and returns an
  `ExportedBundle` (bytes plus SHA-256).
- `Selection::Default` sends the items with `default_checked: true`. The
  withheld `rel_path`s are reported in `SendOutcome::withheld`.

### Errors

`cua_teleport::Error` has these variants:

- `Teleport(TeleportError)`: no provider, unsupported scope, unreadable
  profile, denied OS authorization, or bundle limits.
- `InvalidSelection`
- `NotApproved`
- `Unsupported { app, reason }`: the guest has no importer for the app.
- `Env(cua_spacesd_client::Error)`
- `Task`

The SDK maps them to `CuaError`:

| `cua_teleport::Error` | `CuaError` |
|-----------------------|------------|
| `InvalidSelection` | `InvalidArgument` |
| `NotApproved`, denied authorization | `PermissionDenied` |
| `Unsupported` | `Unsupported` |

## SDK (every language)

With the default-on feature `teleport`, `cua-sdk` exposes this crate over
UniFFI:

```python
tp = cua.teleport()
tp.providers()                                    # [TeleportProvider]
m = await tp.manifest("Slack", TeleportScope.FULL, None)
r = await tp.send(sandbox, "Slack", TeleportScope.FULL,
                  selected_items=None,            # None = default selection
                  approval=MyApproval(),          # TeleportApproval.approve(request) -> bool, or None
                  options=TeleportOptions(chrome_profile=None, display_name=None,
                                          launch_after=None, close_running_app=False))
# send_env(env_client, …) targets an SpacesdClient instead of a Sandbox.
```

The approval callback runs on a worker thread and may block, for example on a
dialog.

## CLI

```sh
cua teleport providers
cua teleport manifest --app Slack [--scope tabs|full] [--profile NAME]
cua teleport push --app Slack --sandbox NAME [--include REL_PATH]... [--all] [--no-launch] [--progress]
cua teleport push --app Slack --url http://HOST:3211 --token T
```

## Built-in apps

The ids come from `cua_teleport_bundle::layout::PROVIDER_IDS`, which is the
same list the receiver imports:

- `chrome`
- `firefox`
- `slack`
- `discord`
- `unity-hub`
- `steam`
- `whatsapp` (macOS)
- `claude-code`

A new app needs changes in three places:

- a layout descriptor in `cua-teleport-bundle/src/layout.rs`;
- an `ExportProvider` here;
- an `ImportProvider` in `cua-spacesd-teleport`.

Both registries assert they match the layout table.

## Host safety

Every host effect goes through `HostEffects`: `pgrep`, `osascript`, Keychain
reads, the DevTools port, Touch ID and `$HOME`.

- `RealHost` refuses all of them under `cfg(test)` or `CUA_ENV_TEST_SANDBOX=1`.
- Tests inject `FakeHost::new().with_home(tempdir)`.
- A source-scan test keeps process and socket APIs inside `host.rs`.
- Tests of crates that embed this one (the SDK and the CLI) must inject a fake
  host. CI also sets `CUA_ENV_TEST_SANDBOX=1`.
