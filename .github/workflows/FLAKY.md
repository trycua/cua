# Known flaky checks

Rule: a flake is fixed at its root cause (race, timeout, port, shared state),
not with retries or raised limits. A check stays here only until its fix
merges. An entry without an owner is a bug in this file.

| Check | Symptom | Root cause | Owner | Status |
|---|---|---|---|---|
| `relay e2e (docker)`: `cua-relay` `tests/metering.rs` `past_the_allowance_the_session_continues_with_an_upgrade_prompt` | fails on slow runners | test slept 300 ms and assumed both streams were counted and open for 50 ms | relay (CUA-1952 fix: PR 93, injectable clock) | fix in review |
| `tag-safety-test` (CD: Image linux) | "tag not found" while the tag is listed | `crane ls \| grep -q` under `pipefail` is killed by SIGPIPE | images (CUA-1952 fix: PR 92) | fix in review |
| `bindings drift` | cancelled at 45 min | cold release build of cua-sdk; now dev profile | SDK (CUA-1952 fix: PR 91) | fix in review |
| `linux-kvm (loaded-stream)` perf guard (PRs 74, 79) | fails once in a while | one sample compared to one old sample with a 2x limit on a shared disk | author of PR 74 / 79 (patch in CUA-1952) | patch ready |
| `handover_process` (`cua-relay`, macOS job) | failed once | unknown: its relay output goes to /dev/null | relay | needs logs; make the harness keep the relay log |
| Tauri bundle (Linux) runner loss | "runner received a shutdown signal" | four rustc jobs on a 7 GB runner (OOM) | Spaces app | fixed: two jobs on Linux |
| `jev-use` macOS native AppKit mock E2E | no `capture_id` | Cua Driver 0.37.0 needs macOS's one-time direct-capture approval; the runner had none | jev-use | fixed (PR 97, public 5665) |
| `jev-use` Linux mock E2E | `could not complete through A-owned authority` | unknown, intermittent on main and PRs | jev-use / cua-driver | open, needs root cause |
| `handover_process` (`cua-relay`) | nothing answered on the port | unknown; relay logs are now kept on failure (PR 96) | relay | waiting for a repro with logs |
| `CD: Image linux` / `omarchy` build | `E: Some index files failed to download` fetching the xtradeb PPA index | launchpad mirror unavailable; the image build has no fallback | images | open |
| `cua-spaces-cluster` `gang_faults` / `pool_agent` tests | members "ended 39 ms apart"; relay count 2 vs 1 | timing assumptions on a 2-vCPU runner | cluster (CUA-1952 follow-up fix) | fix in review |
