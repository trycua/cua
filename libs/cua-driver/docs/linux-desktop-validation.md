# Linux representative desktop validation

Hosted CI owns the canonical Xvfb/Openbox and headless Sway environments. Some
contracts require a real user desktop and therefore run only when a maintainer
explicitly provisions the corresponding environment.

## Environments

| Environment | Required evidence | Current status | Preflight |
| --- | --- | --- | --- |
| GNOME/Mutter | WinRects geometry and activation, portal/libei input, portal recording, and shared renderer apps | Native GTK behavior, capture, and desktop scope are accepted; shared renderers and portal video remain open | Wayland user session, enabled WinRects helper, and portal grant |
| KDE/KWin | KWin-specific activation and portal behavior | Plasma 6 session startup, GTK AT-SPI discovery, and portal interfaces were observed; no behavioral matrix is accepted | Plasma/KWin 6 Wayland session; Plasma 5.27 is rejected |
| Real Xorg | MPX/uinput behavior that Xvfb cannot provide | Not yet validated | Non-Wayland Xorg session with `/dev/uinput` access |
| DRM/EGL renderer | Representative WebKitGTK/Tauri accessibility tree | Not available on the hosted Sway or representative desktop hosts used so far | A real `/dev/dri/renderD128`; software-only headless rendering is rejected |

## Source ownership

The host checkout is the only checkout that commits or pushes. Sync it with:

```bash
libs/cua-driver/scripts/sync-vm-worktree.sh push user@host '~/cua'
```

The sync writes `.cua-e2e-source-sha` after transferring the worktree. The
canonical preflight validates that marker when the VM intentionally has no
`.git` directory, so reports still identify the exact host commit.

## Run

Start the command from the graphical user's systemd user manager or an
equivalent terminal inside that user's session. The wrapper rejects the wrong
desktop generation before building fixtures.

```bash
scripts/ci/linux/run-rust-e2e-desktop.sh gnome
scripts/ci/linux/run-rust-e2e-desktop.sh kde
scripts/ci/linux/run-rust-e2e-desktop.sh xorg
```

The default is the complete canonical matrix. `CUA_E2E_INTERNAL_LANE` and
`CUA_E2E_HARNESS_FILTER` remain diagnostic/maintainer controls; they do not
define a second catalog.

## Evidence acceptance

Behavioral acceptance requires typed rows, the exact source SHA, independent
fixture-state or refusal evidence, and every action-specific desktop oracle. A
representative result without reporter-owned per-cell video may establish that
behavior, but it is recorded as lacking full evidence parity.

Full hosted parity also requires the Markdown summary, screenshots,
trajectories, and per-cell videos enforced by the reporter. Record accepted
behavior and any evidence-parity gap in `action-support.md`. A setup failure is
an environment error, never a smaller green matrix.
