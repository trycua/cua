# Linux execution outcome

## CI proof pending

The end-to-end proof runs in `.github/workflows/e2e-rust-linux.yml`, job `verify-skill`, on pull requests touching `.cursor/skills/verify-cua-driver/**`, `libs/cua-driver/**`, or that workflow. For a manual run, use `workflow_dispatch` with `lane=verify` and optionally a full source SHA in `ref`. Release-call certification lanes are unchanged.

The job has a 25-minute bound, uses its own Rust build cache, saved even on failure, disables retries, and runs the offline checker regressions before invoking the helper. The canonical runner stages Electron from the registry; no runtime is vendored. Evidence is uploaded after cleanup as `verify-cua-driver-evidence`, including failed attempts. Missing artifacts or incomplete evidence fail the job.

On a networked Linux host with the dependencies and toolchain listed in [the skill](SKILL.md), run from the repository root:

```bash
CUA_E2E_RETRY_ONCE=0 .cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11
```

The helper automatically checks its printed evidence directory after desktop teardown. To recheck retained evidence independently:

```bash
.cursor/skills/verify-cua-driver/scripts/check-evidence.sh /absolute/printed/evidence/directory
```

The checker requires both exact delivered/pass summary rows, current-attempt per-cell evidence, preflight diagnostics, and at least one recorded owned PID with none still alive. A helper timeout in CI is a defect to fix under requirement (3), for example by raising its internal bound if build/staging exhausts it; do not add a second pre-build harness.

No successful CI run has been observed yet. Record the run URL and source SHA here only after that job succeeds. No end-to-end attempt was made in the offline sandbox for this change.

## Offline attempts

The offline sandbox cannot fetch Electron from the npm registry, so the proof was never run there; run it on a networked host or in CI.

## Stand-ins

- Xvfb instead of a physical desktop session.
- The real Electron fixture instead of a user's application; it did not launch in these attempts.
- Electron sandbox/GPU disabled, forced accessibility and a fixture CDP port.
- Disposable driver HOME/XDG state, disabled overlay/permissions gate and testkit unrestricted GUI authorization/approval bypass.
- Fixture control only establishes sentinel posture; it does not set the target action result.

These do not certify production admission, interactive consent, physical GPU capture or arbitrary app compatibility. Wayland, macOS and Windows remain unverified.

The typed-SDK cell's click runs in-process and is not recorded. Its `turn-*` screenshots come from the sentinel's activation calls during setup, so the checker only proves paired setup screenshots exist; they do not bracket the SDK click. The click itself is evidenced by `fixture-state.json` (`last_action=left_click`) and the observers. A green CI run does not imply otherwise.
