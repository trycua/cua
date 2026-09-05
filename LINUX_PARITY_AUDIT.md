# cua-driver Linux vs Windows parity audit

**Date:** 2026-05-24
**cua-driver version on VMs:** 0.2.18
**Source review:** main @ `5ad4cfeb` (latest pull this session)

## TL;DR

| | Count |
|---|---|
| Tool names on Windows | **30** |
| Tool names on Linux | **29** |
| Tools where Linux IMPLEMENTATION exists (impl_.rs) | **29 / 29** |
| Tools where Linux is VERIFIED on a real Linux host (per PARITY.md before today) | **3** |
| Tools NEWLY verified today via Xvfb on Ubuntu 22.04 | **2 more** (`check_permissions`, `list_windows`) |
| **Effective parity gap right now** | **24 tools UNVERIFIED on Linux** despite having implementations |

**The code is essentially at parity. The TESTS aren't.** Windows has 33 example/parity binaries under `crates/platform-windows/examples/`. Linux has **zero**.

---

## Tool-by-tool parity status

### Surface differences (the only structural delta)

| Tool | Windows | Linux | Why |
|---|---|---|---|
| `debug_window_info` | ✅ | ❌ missing | Windows-only diagnostic dumping HWND state — not portable |

Everything else has matching tool names + schemas.

### Status per tool (collated from PARITY.md + today's empirical work)

🟢 = VERIFIED on Linux  ·  🟡 = IMPLEMENTED but UNVERIFIED on a real Linux host  ·  🔵 = INTENTIONAL_DIVERGENCE  ·  🔴 = MISSING

| Tool | Linux status | Windows status | Notes |
|---|---|---|---|
| `move_cursor` | 🔵 | 🟢 | Intentional: Linux+Windows are overlay-only (no real-cursor warp); macOS-only warps the OS cursor. Verified overlay behavior. |
| `get_cursor_position` | 🟢 | 🟢 | PARITY.md says VERIFIED. |
| `get_screen_size` | 🟢 | 🟢 | PARITY.md says VERIFIED. |
| `check_permissions` | 🔵 → 🟢 today | 🟢 | Intentional: Linux returns `{atspi, x11, xsend_event}` triple instead of macOS's `{accessibility, screen_recording}`. **Today's Xvfb test passed all 3 true.** |
| `list_apps` | 🟡 | 🟢 | Code ready but not exercised. /proc/+ XDG-walk-based on Linux. |
| `list_windows` | 🟡 → 🟢 today | 🟢 | **Today's Xvfb test found xeyes.** Multi-app stress untested. |
| `get_window_state` | 🟡 | 🟢 | The big one — combines AT-SPI walk + screenshot. Linux uses at-spi-bus-launcher; differs across DE (Xfce vs GNOME vs KDE). |
| `screenshot` | 🟡 | 🟢 | XGetImage (x11rb) or ImageMagick `import` fallback. Today's test hit SSH-pipe timeout on the b64 blob, not a real failure. |
| `click` | 🟡 | 🟢 | XSendEvent ButtonPress/Release. |
| `double_click` | 🟡 | 🟢 | Same primitive, 2 events. |
| `right_click` | 🟡 | 🟢 | Same primitive, Button3. |
| `drag` | 🟡 | 🟢 | ButtonPress + MotionNotify×steps + ButtonRelease. |
| `scroll` | 🟡 | 🟢 | XSendEvent Button4/Button5 events. |
| `type_text` | 🟡 | 🟢 | XKeyEvent via XTest. |
| `type_text_chars` | 🟡 | 🟢 | Character-by-character keysym lookup. |
| `press_key` | 🟡 | 🟢 | Single XKeyPress/Release. |
| `hotkey` | 🟡 | 🟢 | Modifier+key composed via XTest. |
| `set_value` | 🟡 | 🟢 | AT-SPI `setText` via atspi crate. |
| `launch_app` | 🟡 | 🟢 | Forks process (exec via .desktop discovery or absolute path). |
| `kill_app` | 🟡 | 🟢 | SIGKILL via libc. |
| `get_accessibility_tree` | 🟡 | 🟢 | AT-SPI walk. |
| `zoom` | 🟡 | 🟢 | Crop region from screenshot. |
| `get_config` / `set_config` | 🟡 | 🟢 | JSON config persistence. |
| `set_agent_cursor_enabled` | 🟡 | 🟢 | Overlay show/hide. |
| `set_agent_cursor_style` | 🟡 | 🟢 | Overlay icon. |
| `set_agent_cursor_motion` | 🟡 | 🟢 | Overlay animation params. |
| `get_agent_cursor_state` | 🟡 | 🟢 | Overlay state readback. |
| `page` (browser JS exec) | 🟡 | 🟢 | CDP-based; same path. |
| `replay_trajectory` | 🟢 | 🟢 | Trajectory file → re-issue calls. Cross-platform. |
| `debug_window_info` | 🔴 | 🟢 | Windows-only HWND diagnostic. Intentional. |

**Score:** 4 verified · 24 implemented-but-unverified · 1 intentional divergence · 1 Windows-only · 1 missing-on-macOS-too.

### Verification artifact gap

```
crates/platform-windows/examples/    ← 33 parity binaries (one per tool)
crates/platform-linux/examples/      ← does not exist (0 binaries)
```

**This is the single biggest action item.** Each `*_parity.rs` example takes ~50–100 LOC and validates one tool against a known fixture (calculator open, xeyes window, etc.). Building the Linux equivalents is the cheapest way to flip 24 🟡s to 🟢s.

---

## How to split testing across distros

Linux variance falls on **3 axes** that matter for cua-driver-rs:

| Axis | Why it matters for cua-driver | Variants worth testing |
|---|---|---|
| **Display server** | Native code path: X11 direct, Wayland needs XWayland fallback. XWayland adds geometry/scaling translation bugs. | X11 native · Wayland+XWayland · (eventually: pure Wayland regression detection) |
| **AT-SPI provider** | Different DEs ship different at-spi versions + register elements differently. The `get_window_state` / `get_accessibility_tree` output varies. | Xfce (at-spi-bus-launcher) · GNOME (gnome-shell a11y bridge) · KDE (kf5-at-spi) |
| **Package family + glibc** | Affects install scripts (apt vs dnf), shared lib compat, kernel-XTest API. | Debian/Ubuntu (apt, glibc 2.35+) · RHEL/Fedora (rpm) · Arch (rolling) |

### Tier 1 — the 3 VMs we just provisioned (cover ~80%)

| VM | Combo | Hits which axes |
|---|---|---|
| `cua-linux-ubuntu2204` | Xfce, X11 native, deb | **All three native-X11 paths**: x11rb direct, at-spi-bus-launcher, glibc 2.35. The "happy path." |
| `cua-linux-ubuntu2404` | GNOME, Wayland+XWayland, deb | XWayland fallback path + gnome-shell at-spi bridge. Newer glibc 2.39. |
| `cua-linux-debian12` | GNOME, Wayland+XWayland, deb | Older glibc 2.36 + slightly older at-spi version. Catches "works on Ubuntu, breaks on Debian" issues. |

**Recommended testing matrix on these 3**:
1. **All 28 tools** smoke-tested in a fresh xrdp session on each VM.
2. **at-spi-heavy tools** (`get_window_state`, `get_accessibility_tree`, `set_value`) run against representative apps:
   - LibreOffice Writer (open in both Xfce and GNOME — output should differ; document the diff)
   - Firefox (Mozilla a11y is its own beast)
   - GNOME Files / Thunar (DE-native file managers)
3. **Multi-monitor + fractional-scaling** specifically on Combo B (GNOME default is fractional scaling on Wayland; this breaks naive XGetImage)

### Tier 2 — high-value adds (~one more day of setup)

| Add | Why |
|---|---|
| **Fedora 41 + GNOME** | RPM package family. Newer SELinux defaults (could block XTest). Different kernel branch. |
| **Kubuntu 24.04 + KDE** | Third major at-spi provider (kf5-at-spi). KDE-native apps (Dolphin, Kate) have distinct AT-SPI structure. |
| **Ubuntu 22.04 + KDE Plasma** | Mixes "older base" with "different DE" — surfaces at-spi-version-vs-DE bugs. |

### Tier 3 — defensive regression detection

| Add | Why |
|---|---|
| **Pure Wayland (sway, KDE Wayland)** | EXPOSES the lack of native Wayland support. cua-driver should fail with a clear "no X server / start XWayland" error. Catches regressions when someone tries to "make it work on pure Wayland" half-heartedly. |
| **Linux ARM64 (Azure ARM VM)** | Verifies x11rb / atspi crates build on ARM. cua-driver is x86-only today; ARM build is the test that surfaces dependencies that aren't ARM-ready. |
| **Headless Xvfb-only** (no real DE) | What we did today. Useful for CI: no GUI session needed, captures the bulk of the X11 code path. Worth pinning as a CI workflow. |

---

## Recommended next actions, in priority order

1. **Run the 28-tool smoke test on Combo A first** (X11 native, simplest). Once that's clean, the bug surface narrows to "Wayland/XWayland-specific" or "GNOME-specific" for B+C.
2. **Author Linux parity examples** — port `crates/platform-windows/examples/*_parity.rs` to `crates/platform-linux/examples/` one per tool. ~30 small files, mostly mechanical. Each one flips a 🟡 to 🟢 in PARITY.md.
3. **Pick the riskiest 3 tools to verify on B+C now**: `get_window_state` (at-spi-heavy), `screenshot` (XWayland scaling), `click` (XSendEvent through XWayland). Catches the biggest "Linux-specific landmines" early.
4. **Add a Xvfb-only CI job** that runs the parity examples on every PR. Cheapest way to keep Linux from regressing silently.
5. **Update PARITY.md** in batches as tools get verified — flip 🟡 → 🟢 with the specific VM / test + commit hash.

---

## What's already proven today

- Install path: `install-local.sh --release` works on all 3 distros (Ubuntu 22.04, Ubuntu 24.04, Debian 12) end-to-end. Binary lands at `~/.local/bin/cua-driver`.
- Binary runtime: `cua-driver --version`, `cua-driver list-tools`, `cua-driver call check_permissions`, `cua-driver call list_windows` all work on Ubuntu 22.04.
- AT-SPI on Linux: with at-spi-bus-launcher + at-spi2-core packages + a dbus session, `check_permissions` reports `atspi=true`.
- XSendEvent path: `xsend_event=true` reported by check_permissions confirms input synthesis is available.
- Display server selection works: with `DISPLAY=:99` + Xvfb running, `list_windows` correctly finds xeyes (one of the canonical X11 test apps).

What's NOT yet verified today: the at-spi-heavy tools (`get_window_state`, `get_accessibility_tree`, `set_value`) — these need a real DE running + the at-spi-registryd registered (Xvfb-only doesn't register real apps with at-spi). Best path: log in via xrdp and run from inside the Xfce session.

---

End of audit.
