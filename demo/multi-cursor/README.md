# Multi-cursor background computer-use demo — "National Records System"

A fleet of deliberately **legacy-looking government records terminals** (navy
banner, `UNCLASSIFIED // FOR OFFICIAL USE ONLY` strip, function-key bar,
green-screen records grid, status line) — the kind of internal agency app that,
in the age of AI, has *no* automation integration. cua-driver automates them
anyway.

One human action in the foreground "master" terminal is replayed onto **four
background terminals at the same time**, each driven by its own cua-driver
session = its own uniquely-coloured agent cursor — with **no window ever
raised** and **the user's mouse never moved**.

It also proves cua-driver works **with or without an accessibility tree**: the
five windows span five UI frameworks, and cua-driver's default dispatch
auto-selects UIA-Invoke where an a11y tree exists and falls back to
pixel/pointer-injection where it doesn't.

## Layout (each window = ½ work-width × ½ work-height)

The four corners tile the taskbar-safe work area into quadrants; the master is
centered, **overlapping all four**:

```
 ┌────────────────────────┬────────────────────────┐
 │ Win32 GDI (NO a11y)     │ WinForms (.NET)         │
 │            crimson ●    │            amber ●      │
 │           ┌────────────────────────┐             │
 │           │ MASTER — Win32 controls │  ← you      │
 ├───────────│ (foreground, overlaps)  │─────────────┤
 │ WPF (XAML)│                         │ Electron    │
 │           └────────────────────────┘ mint_lime ●  │
 │            aqua ●       │            (Chromium)    │
 └────────────────────────┴────────────────────────┘
```

Click **SUBMIT** (or type a subject name then submit) in the center master:
four coloured cursors glide onto the four corner terminals and commit the same
record there — concurrently, in the background. Watch each corner's
green-screen records grid grow and its `RECORDS:` counter tick up, without any
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
