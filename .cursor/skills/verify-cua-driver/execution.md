# Linux execution

## CI proof

The end-to-end proof runs in `.github/workflows/e2e-rust-linux.yml`, job `verify-skill` ("Linux / verification skill proof"), on pull requests touching `.cursor/skills/verify-cua-driver/**`, `libs/cua-driver/**`, or that workflow. For a manual run, use `workflow_dispatch` with `lane=verify` and optionally a full source SHA in `ref`. Release-call certification lanes are unchanged.

The job has a 25-minute bound, shares the Rust build cache, disables retries, and runs the offline checker regressions before invoking the helper. The canonical runner stages Electron from the registry; no runtime is vendored. Evidence is uploaded after cleanup as `verify-cua-driver-evidence`, including failed attempts. Missing artifacts or incomplete evidence fail the job. The first recorded pass is [run 42399619203](https://github.com/trycua/cua/actions/runs/42399619203).

On a networked Linux host with the dependencies and toolchain listed in [the skill](SKILL.md), run from the repository root:

```bash
CUA_E2E_RETRY_ONCE=0 .cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11
```

The helper automatically checks its printed evidence directory after desktop teardown. To recheck retained evidence independently:

```bash
.cursor/skills/verify-cua-driver/scripts/check-evidence.sh /absolute/printed/evidence/directory
```

The checker reads the cells the run declared in `cases.jsonl`. Each declared cell needs exactly one summary row with its expected outcome (`Delivered` or `Refused`) and `Pass`, plus current-attempt per-cell evidence. Background cells also need sentinel and desktop-observation evidence with a heartbeat, and delivered click cells need their `last_action` marker. It also requires preflight diagnostics, and at least one recorded owned PID with none still alive. A helper timeout in CI is a defect to fix, for example by raising its internal bound if build/staging exhausts it; do not add a second pre-build harness.

## Stand-ins

- Xvfb instead of a physical desktop session.
- The real Electron fixture instead of a user's application.
- Electron sandbox/GPU disabled, forced accessibility and a fixture CDP port.
- Disposable driver HOME/XDG state, disabled overlay/permissions gate and testkit unrestricted GUI authorization/approval bypass.
- Fixture control only establishes sentinel posture; it does not set the target action result.

These do not certify production admission, interactive consent, physical GPU capture or arbitrary app compatibility. Wayland and Windows remain unverified here; macOS runs in its own [Lume lane](macos-lume.md).
