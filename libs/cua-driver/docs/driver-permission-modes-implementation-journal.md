# Driver Permission Modes Implementation Journal

## Status

- **Goal:** Implement the reviewed driver permission-mode and consent plan.
- **Branch:** `codex/cua-driver-permission-modes`
- **Frozen base:** `origin/main` at
  `767acf25f25ea668ffab428e4f2e8985896de98e`
- **Source version:** Cua Driver `0.9.1`
- **Umbrella issue:** [#2381](https://github.com/trycua/cua/issues/2381)
- **Plan:** [driver-permission-modes-and-consent-plan.md](driver-permission-modes-and-consent-plan.md)

This journal records exact-head implementation and validation evidence. A row
is not called supported until the named public interface, independent oracle,
and representative environment have passed on the same commit.

## Frozen delivery slices

1. Fail-closed policy startup and one canonical daemon authorization path.
2. Immutable permission modes, policy composition, risk metadata, and status.
3. Protected-host approval broker and generalized grants.
4. Persistent indicator, Stop/revoke, and existing-profile migration.
5. Autonomous manifests and explicit unrestricted startup.
6. Driver-wide capability adapters, documentation, and platform certification.

Each slice remains on this host-owned branch until it is independently tested
and can be split into a reviewable PR without losing dependency history.

## Support and evidence matrix

| Environment | Public surface | Required result | Evidence state |
|---|---|---|---|
| macOS host | Rust unit/integration tests | Deterministic policy, mode, grant, and refusal contracts | Package gates passed; full workspace examples have a pre-existing macOS cross-target failure recorded below |
| macOS Lume Aqua session | Installed `cua-driver` CLI/MCP/daemon | Mode/status/revoke, fail-closed protected attach, unrestricted lifecycle, provenance | Pending exact-commit certification |
| Windows interactive session (not Session 0) | Installed CLI/MCP/daemon and Windows runner | Mode/policy parity; advertised collector and revocation behavior | Pending implementation |
| Linux X11 | Installed CLI/MCP/daemon and Linux harness | Mode/policy parity and selected collector behavior | Pending implementation |
| Linux Wayland | Installed CLI/MCP/daemon and Wayland harness | Mode/policy parity and selected collector behavior | Pending implementation |
| Headless transport | CLI/raw socket/MCP | Stable refusal when no certified trusted-consent collector exists | Pending implementation |

## Execution log

### 2026-07-20 — orientation and baseline

- Fetched `origin/main`; local base and fetched main both resolve to
  `767acf25f25ea668ffab428e4f2e8985896de98e`.
- Preserved the previously reviewed, uncommitted plan and created
  `codex/cua-driver-permission-modes` from that exact commit.
- Confirmed the first P0 gap in production code: a configured missing policy
  path logs a warning and becomes `Ok(None)`, while daemon binding happens
  before policy validation.
- Confirmed policy checks are duplicated in core stdio MCP, daemon socket, and
  stdio proxy dispatch.
- Baseline commands passed:
  - `cargo test -p cua-driver-core policy --all-features` — 13 passed.
  - `cargo test -p cua-driver --bin cua-driver policy --all-features` — build
    passed; no matching binary tests.

### 2026-07-20 — containment and immutable startup modes

- Explicitly configured missing/invalid policy paths now fail before Unix or
  Windows daemon bind, and the stdio proxy validates before beginning MCP
  dispatch.
- Core stdio, direct daemon, and proxy paths use one canonical capability
  policy function; the daemon remains authoritative and the proxy is only an
  early-denial optimization.
- Added immutable startup modes (`standard`, `autonomous`, `unrestricted`),
  with `standard` as the default and a separately named danger acknowledgement
  required for `unrestricted`.
- Disabled the same-user-writable existing-profile artifact in production by
  default. Standard/autonomous now return `browser_consent_required`; the
  legacy path requires an explicit trusted-launcher compatibility flag.
- Added direct CLI/daemon adversarial coverage for a syntactically valid forged
  artifact and for unrestricted launch-time behavior.
- Verification passed:
  - `cargo test -p cua-driver-core policy --all-features` — 13 passed.
  - `cargo test -p cua-driver-core authorization --all-features` — 5 passed.
  - `cargo test -p cua-driver-core browser::v2_tests --all-features` — 29 passed.
  - `cargo test -p cua-driver --test permission_policy_startup_test --all-features`
    — 3 passed.
  - Both new `daemon_required_test` authorization cases passed individually.
  - `cargo test -p cua-driver --bin cua-driver cli::tests --all-features` —
    13 passed.
  - `cargo fmt --all -- --check` — passed.

### 2026-07-20 — protected-host contract spike

- Current Codex documentation exposes MCP elicitation/approval UX, optional
  automatic approval review, and desktop-host attestation on Codex's app-server
  protocol. It does not define a verifiable Cua MCP-server attestation that
  proves a human (rather than a hook/reviewer/client) accepted an elicitation.
- MCP 2025-11-25 likewise leaves the client interaction model open and warns
  servers not to trust unverified client-provided identity.
- Decision: generic `elicitation/create` remains untrusted. The broker will
  require a request-bound authenticated provider result and otherwise fail
  closed. No Codex or Claude collector will be advertised as protected until a
  concrete host contract can be verified end to end.
- The live repository already has a versioned dotted capability vocabulary in
  `tool.rs`. Risk metadata will extend that canonical map rather than create a
  competing manifest vocabulary.

### 2026-07-20 — modes, broker, grants, and bounded autonomy

- Added the reviewed R0 through R4 risk map to every advertised tool. Unknown
  tools fail closed. `browser_prepare.existing_profile` is the first active
  resource adapter; other tools are explicitly marked `metadata_only`.
- Added immutable managed and user policy layers with SHA-256 provenance and
  intersection semantics. Either explicitly configured invalid path now fails
  before the action socket binds.
- Added request-bound protected consent with daemon instance, nonce,
  generation, mode, policy hashes, exact sessions, resource, expiry, and digest.
  Generic MCP elicitation, files, TTY input, and tool arguments cannot implement
  the provider seam.
- Made indicator activation a prerequisite for protected grants. Stop, exact
  session teardown, expiry, reconnect exhaustion, and context drift revoke the
  grant and release its browser connection.
- Added `cua-driver revoke --session <id>` and `--all`, plus content-free mode,
  policy, provider, and manifest status.
- Added autonomous manifests with a 24-hour maximum, explicit launch approval,
  deny-by-default tool sets, exact existing-profile PID/window scope, requested
  origin checks, and live top-level origin checks before each browser mutation.
- Added unrestricted startup warnings and
  `CUA_DRIVER_DISABLE_UNRESTRICTED=1` for administrator disablement.
- Added a hard refusal for PID-targeted tools that name the authorization
  daemon itself.
- Updated generated CLI/MCP references, policy concepts and how-to pages,
  browser attachment docs, test matrix, and a new permission-mode reference.

Verification on the macOS source host:

- Core `policy`, `authorization`, `consent`, `session_manifest`, grant, session,
  refusal, and browser v2 suites passed in targeted runs.
- `permission_policy_startup_test`: 8 passed.
- `daemon_required_test`: 7 passed.
- CLI manifest/parser tests: 13 passed.
- `cargo test -p cua-driver-core --no-default-features authorization`: 8 passed.
- Generated documentation drift, public hygiene, and internal link checks
  passed.
- `cargo test --workspace --all-features` is not a valid all-host gate on
  macOS: Linux-only examples `libei_input` and `screenshot_cascade` compile
  against modules gated by `target_os=linux`. The failure occurred before test
  execution and is unrelated to this change. Package-scoped and target-native
  VM gates remain required.

### 2026-07-20 — independent Fable implementation review

- Claude Code Fable reviewed the uncommitted implementation against the plan
  and returned **SHIP AFTER FIXES** with no P0 findings.
- Closed every P1 finding before platform certification:
  - enforced a terminal autonomous idle timeout at canonical dispatch;
  - rejected origin-scoped manifests that also allow generic page/desktop
    input surfaces outside the typed live-origin adapter;
  - made the legacy artifact startup-invalid in autonomous and unrestricted
    modes, preserving the mandatory autonomous indicator;
  - extracted and directly tested the live-origin decision path used after
    `Page.getFrameTree` and before browser mutation.
- Closed the bounded P2 hardening items in the same pass: provider request and
  indicator deadlines, final expiry checks, provider identity in status,
  metadata-only old-daemon proxy fallback, change-during-load policy hashing,
  and explicit indicator-lease teardown semantics.
- Corrected documentation so `standard` describes only the currently active
  existing-profile prompt, and documented PID reuse, origin-adapter exclusions,
  terminal idle expiry, and restart-flag preservation.
- Post-review targeted gates passed:
  - session-manifest tests: 5 passed;
  - consent tests: 6 passed;
  - live-origin decision test: 1 passed;
  - `permission_policy_startup_test`: 9 passed.

## Current work

Run the package-level macOS gate, freeze a commit, install that exact commit in
the Lume VM and interactive Windows/Linux lanes, then publish the branch and
open the review PR. No standalone host is advertised as a certified protected
provider; standard/autonomous existing-profile attachment remains fail closed.
