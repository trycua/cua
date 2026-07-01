# cua-driver docs audit (macOS / Linux / Windows)

Autonomous audit started 2026-07-01. Method: walk the docs as a human would —
install → tutorial → how-to guides → recipes → reference — running every
documented command against live cua-driver 0.7.0, on each platform, and record
mistakes, things that don't make sense, and inconsistencies. Fixes land on the
`docs/driver-audit-fixes` branch as they're confirmed.

Ground truth: cua-driver 0.7.0, 38 MCP tools.

## Status

| Platform | Install | Tutorial | How-to | Recipes | Reference |
|---|---|---|---|---|---|
| macOS (local) | ✅ | ✅ | ✅ | ✅ | ✅ |
| Linux (Azure) | ✅ | gui¹ | ✅ | gui¹ | ✅ |
| Windows (Azure) | — | — | — | — | — |

¹ GUI-driving legs not run live: a stock Ubuntu server has no desktop, and the
docs never say how to bootstrap one. Verified CLI + doc-consistency instead.

macOS how-to guides largely clean (verified live: drove a real Chrome tab through
the `page` tool; cursor/recording/modality flags match `describe`). Fixes below.

Severity: **[blocker]** wrong + blocks a user · **[confusing]** works but
misleads · **[inconsistent]** disagrees with another doc or the CLI ·
**[polish]** small.

## Findings — macOS (iteration 1, live v0.7.0)

1. **[blocker] tutorials/drive-your-first-app.mdx §5** — said "The agent reports
   391" while step 4 (and 6×7) is 42; live drive read 42. Self-contradiction.
   → FIXED (391 → 42).
2. **[inconsistent] how-to-guides/driver/install.mdx §Verify** — `--version` and
   `doctor` printed `0.5.x`; live is 0.7.0. → FIXED (both → 0.7.0).
3. **[inconsistent] how-to-guides/driver/install.mdx §Grant TCC** — the ✅
   "Accessibility/Screen Recording: granted" pretty output was shown under
   `cua-driver check_permissions`, but that command returns JSON; the pretty
   format is `cua-driver permissions status`. → FIXED (command → `permissions
   status`).
4. **[inconsistent] tutorials/drive-your-first-app.mdx §3 (Claude Code tab)** —
   the live registration helper emits `claude mcp add-json --scope user … {"args":["mcp"]…}`
   (adds `--scope user`, omits `--claude-code-computer-use-compat`). The flag is
   intentionally documented in connect-your-agent.mdx (kept for future
   compat-gated tools), so tutorial + that guide agree with each other but drift
   from the live helper. → DEFERRED: needs a maintainer decision on whether the
   helper or the docs are canonical; do not strip the flag blindly. Verify
   whether Claude Code now requires `--scope user`.
5. **[polish] install.mdx §Verify** — the `doctor` sample elides real lines
   (argv exe/resolved, telemetry, legacy LaunchAgent, TCC+cdhash) and shows an
   `.local/bin` install dir; a bundle install reports `/Applications/CuaDriver.app/…`.
   Illustrative, low impact. → note; consider marking the sample abbreviated.
6. **[polish] tutorials §4** — element_index values in the sample transcript
   don't match a live snapshot (indices vary per snapshot). Consider a
   "indices vary" note.

### Omissions a first-time user hits (macOS)
- Tutorial never starts the daemon (`open -n -g -a CuaDriver --args serve`);
  install.mdx does. The step-4 element_index drive silently depends on a
  persistent daemon. → add a one-line callout.
- Every button `click` returns `effect: unverifiable, verified: false` (a press
  has no AX post-condition); correctness only appears on the re-snapshot. Raw
  CLI watchers may think clicks failed. → worth a sentence.
- Driving needs Screen Recording; a user who dismisses that TCC dialog hits
  failures the tutorial never mentions.

### Verified working verbatim (coverage)
`--version`, `doctor` (all [ok]), `permissions status` (+`--json`),
`check-update` (+`--json`, on-latest nulls correct), `status`, `call list_apps`,
`list-tools`, and the full drive loop: start_session → launch_app Calculator
(backgrounded, `self_activation_suppressed`) → get_window_state (full AX tree) →
5× click by element_index → re-snapshot → read 42. No-foreground contract holds.

## Findings — cross-doc consistency (iteration 1, vs live CLI)

Tool inventory fully in sync (all 38 documented, none missing/extra); no broken
internal *page* links. Issues:

7. **[confusing] mcp-tools.mdx §set_value** — references `type_text_chars`, which
   is not one of the 38 tools; the real tool is `type_text`. Copied verbatim from
   the binary's `describe set_value`, so the bug is UPSTREAM (Rust tool
   description). → SOURCE FIX needed; mcp-tools.mdx is auto-generated.
8. **[confusing] capture-and-dispatch-modalities.mdx / windows-ssh.mdx** — phantom
   `screenshot` tool (no such tool; connect-your-agent correctly says so).
   → FIXED (removed both mentions).
9. **[confusing] capture-and-dispatch-modalities.mdx:63** — anchor pointed at
   `mcp-tools#action-response-shape`; that section is in `mcp-tool-notes`.
   → FIXED.
10. **[inconsistent] explanation/index.mdx** — "three axes" vs the page's "four".
    → FIXED (three → four, added the rung axis).
11. **[inconsistent] demonstrations…mdx** — line 30 "video off by default" vs
    table "recording.mp4 by default"; live confirms off. → FIXED (table).
12. **[inconsistent] connect-your-agent.mdx Pi section** — `cua-driver list_apps`
    / `cua-driver click …` should be `cua-driver call …`. → FIXED.
13. **[inconsistent] connect-your-agent.mdx mcp-config claude example** — stale;
    live emits `--scope user` and omits `--claude-code-computer-use-compat`.
    → FIXED (example + explanation; corroborates macOS finding 4).
14. **[inconsistent] cli-reference.mdx** — missing `manifest` subcommand; root
    cause `dump-docs` omits it. → GENERATOR FIX needed.
15. **[inconsistent] cli-reference.mdx §mcp flag** — description overstates the
    now-inert `--claude-code-computer-use-compat`. → GENERATOR/SOURCE FIX.
16. **[polish] cli-reference / personalize-cursor** — serve/mcp cursor + pip
    flags undocumented (`--no-overlay`, `--cursor-id/icon/shape/palette`,
    `--experimental-pip*`); `dump-docs` reports `serve args: []`. → GENERATOR FIX.
17. **[inconsistent] choose-a-modality.mdx title vs inbound link text** — real
    title "Choose an Action Rung and Dispatch Mode"; several docs link it as
    "Choose a capture and dispatch mode". → pending (multi-file link-text unify).
18. **[polish] stale sample versions** — update.mdx (0.3.x), install/recipes
    (0.5.x), windows-ssh (0.2.7 floor) vs live 0.7.0. → pending.
19. **[polish] no-foreground-contract page** — file title "Background
    Computer-Use" but linked under 2–3 different names. → pending.

## Fixes applied
- tutorials/drive-your-first-app.mdx: 391 → 42.
- how-to-guides/driver/install.mdx: version 0.5.x → 0.7.0 (×2); TCC check
  command `check_permissions` → `permissions status`.
- capture-and-dispatch-modalities.mdx: fixed the action-response-shape anchor
  (→ mcp-tool-notes); removed the phantom `screenshot`-tool parenthetical.
- explanation/index.mdx: three axes → four (added the action-rung axis).
- demonstrations-skills-and-trajectories.mdx: video is not recorded "by default".
- windows-ssh.mdx: removed `screenshot` from the sample tool list.
- connect-your-agent.mdx: Pi examples use `cua-driver call …`; mcp-config claude
  example updated to live output (`--scope user`, no compat flag) + explanation.
- recipes (fill-a-form, build-a-report, export-contacts): `call check_permissions`
  (returns JSON) → `permissions status` (matches the shown ✅ output); recipe
  version placeholder `0.5.x` → `0.7.0`.
- keep-running.mdx: the macOS autostart helper is repo-relative and fails for
  curl-installed users — reframed so the plist is the path without a checkout.
- personalize-cursor.mdx: documented the live `--cursor-palette <name>` launch
  flag (was missing from the cursor-personalization guide).
- install.mdx (Linux): **[blocker]** fresh Ubuntu 22.04 can't launch the binary
  (`libXi.so.6` missing) — added `sudo apt install libxi6 at-spi2-core` prereq
  with the sudo caveat + a note that headless servers have no desktop to drive;
  dropped the phantom "ffmpeg" claim from the `doctor` sentence (no ffmpeg probe).
- keep-running.mdx (Linux): headless boxes have no `graphical-session.target` —
  noted to use `WantedBy=default.target` there.

## Findings — Linux (Azure Ubuntu 22.04, live v0.7.0)

20. **[blocker] install.mdx Linux** — missing `libXi.so.6` on stock Ubuntu; docs
    list no apt prereqs and claim "no admin access". → FIXED.
21. **[inconsistent] mcp-tools.mdx:15 — tool count is platform-specific.** Linux
    `list-tools` returns **43**, not 38; the 5 Linux-only tools are
    `mouse_button_down`, `mouse_button_up`, `mouse_drag`, `parallel_mouse_drag`,
    `type_text_chars`. This resolves the earlier `type_text_chars` puzzle (exists
    on Linux, not macOS). mcp-tools.mdx is auto-generated from one platform and
    documents 38. → GENERATOR FIX: emit a per-platform tool surface (or note the
    Linux-only tools) + correct the count. Coordinate with cua#2088. The
    `set_value` "prefer type_text_chars" line is then correct on Linux, wrong on
    macOS — needs a platform qualifier.
22. **[inconsistent] install.mdx §doctor** — sentence claims doctor checks
    "ffmpeg"; it has no ffmpeg probe. → FIXED.
23. **[polish] keep-running Linux systemd** — `graphical-session.target` vs the
    headless case. → FIXED.
24. **[confusing] install.sh post-install output (script, not doc)** — Linux
    "Next steps" are macOS-centric (TCC/CuaDriver.app). → eng: make installer
    output platform-aware.

### Linux verified accurate
Install path/layout exact; full serve/status/stop lifecycle after `libxi6`;
`check_permissions` honest Linux JSON; `install_ffmpeg` report-only;
`health_report` platform_supported=pass; explanation/linux-and-wayland.mdx claims
(AT-SPI dependency, X11/Wayland detection, graceful headless `doctor` warnings)
all hold.

Azure VM torn down (RG `cua-docs-audit` delete accepted).

## Open / deferred
- SOURCE/GENERATOR fixes (need cua-driver Rust / dump-docs changes, since the
  reference MDX is auto-generated): #7 `type_text_chars`→`type_text` in set_value
  description; #14 add `manifest` to dump-docs; #15 compat-flag description; #16
  undocumented serve/mcp flags. Coordinate with cua#2088 (reference auto-gen).
- Hand-doc polish pending: #17 link-text unify, #18 sample-version refresh, #19
  page-name drift; plus macOS omissions (daemon-start callout, unverifiable
  clicks, screen-recording dependency).
- Linux + Windows (Azure) — Linux leg in progress.

## Azure cleanup (wind-down MUST run)
All audit VMs live in resource group **`cua-docs-audit`**. To tear everything
down: `az group delete -n cua-docs-audit --yes --no-wait`.
