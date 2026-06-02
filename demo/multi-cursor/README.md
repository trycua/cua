# Multi-cursor background computer-use demo

Shows off cua-driver's Windows background actuation: **one human action in a
foreground window is replayed onto four background windows at the same time**,
each driven by its own cua-driver session = its own uniquely-coloured agent
cursor — with **no window ever raised** and **the user's mouse never moved**.

It also proves cua-driver works **with or without an accessibility tree**: the
five windows span five UI frameworks, and cua-driver's default dispatch
auto-selects UIA-Invoke where an a11y tree exists and falls back to
pixel/pointer-injection where it doesn't.

## Layout (2×2 + center)

```
 ┌───────────────┐                 ┌───────────────┐
 │ Win32 GDI      │   crimson ●     │ WinForms       │   amber ●
 │ (NO a11y tree) │                 │ (.NET classic) │
 └───────────────┘                 └───────────────┘
                 ┌───────────────┐
                 │ MASTER         │  ← you click / type here (foreground)
                 │ (Win32 ctrls)  │
                 └───────────────┘
 ┌───────────────┐                 ┌───────────────┐
 │ WPF            │   aqua ●        │ Electron       │   mint_lime ●
 │ (XAML / UIA)   │                 │ (Chromium)     │
 └───────────────┘                 └───────────────┘
```

The four corners are background windows. When you click **SUBMIT** (or type a
name and submit) in the center master, four coloured cursors glide onto the
four corners and perform the same action there — concurrently, in the
background. Watch the corners' "Clicks:"/"Last:" lines update without any
corner ever coming to the front.

## Frameworks (and what they exercise)

| Window | Framework | Accessibility | cua-driver path |
|---|---|---|---|
| TL | Win32 + GDI (custom-drawn) | **none** | pixel hit-test → PostMessage / pointer injection |
| TR | .NET WinForms | MSAA/UIA | UIA Invoke |
| BL | .NET WPF | UIA (XAML) | UIA Invoke (no foreground steal via WS_EX_NOACTIVATE) |
| BR | Electron | UIA (Chromium) | UIA Invoke |
| Center | Win32 standard controls | MSAA | (foreground; the human drives it) |

## Build

```powershell
# from this directory
cargo build                                   # legacy-app + orchestrator (Rust)
dotnet build dotnet/winforms/winforms.csproj  # WinForms
dotnet build dotnet/wpf/wpf.csproj            # WPF
npm install --prefix electron                 # Electron (downloads electron once)
```
Also build the driver once (repo root workspace):
```powershell
cargo build -p cua-driver --manifest-path ..\..\libs\cua-driver\rust\Cargo.toml
```

## Run

```powershell
.\target\debug\orchestrator.exe            # human-driven: click/type in the center
.\target\debug\orchestrator.exe --auto     # self-playing: drives a TYPE+CLICK every few seconds
```

The orchestrator starts the cua-driver daemon, launches + positions all five
windows, and fans every center action out to the four corners over four
concurrent `cua-driver call` sessions (`crimson` / `amber` / `aqua` /
`mint_lime` → four cursor colours). Close the center window (or kill the
orchestrator) to tear everything down — a Windows Job Object kills the whole
tree, so nothing is orphaned.

### Env overrides
`CUA_DRIVER_EXE`, `LEGACY_APP_EXE`, `WINFORMS_EXE`, `WPF_EXE`, `ELECTRON_DIR`.

## How the coloured cursors work
cua-driver assigns each session a cursor colour by name (palette-name sessions
like `crimson` pick that colour directly). Passing `"session":"<color>"` on each
`click`/`type_text` call routes it to that session's overlay cursor, which
glides to the target. Four sessions → four cursors animating at once. See
`docs/windows-background-input-re-plan.md` for the no-z-raise mechanism.
