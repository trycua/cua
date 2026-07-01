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
| macOS (local) | ✅ audited | ✅ audited | — | — | in progress |
| Linux (Azure) | — | — | — | — | — |
| Windows (Azure) | — | — | — | — | — |

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

## Fixes applied
- tutorials/drive-your-first-app.mdx: 391 → 42.
- how-to-guides/driver/install.mdx: version 0.5.x → 0.7.0 (×2); TCC check
  command `check_permissions` → `permissions status` to match its shown output.

## Open / deferred
- Finding 4 (mcp-config helper vs docs) — needs maintainer decision.
- Findings 5, 6 and the three omissions — pending the next fix pass.
- Consistency audit (agent B) — in progress.
- Linux + Windows (Azure) — not started.
