# Known flaky checks

Rule: a flake is fixed at its root cause (race, timeout, port, shared state),
not with retries or raised limits. A check stays here only until its fix
merges. An entry without an owner is a bug in this file.

While the fix is in progress, a flake can be quarantined in
[`.github/flaky/quarantine.json`](../flaky/quarantine.json), with an owner, an issue
and an expiry (7 days for a new flake, never more than 90):

- A `test` entry: the test runner retries the test once, and a quarantined
  failure is reported but does not fail its job.
- A `job` entry (check name glob, workflow name glob and a `signature` regex
  from the failure log): `ci-gate` reruns the failed jobs once. If they fail
  again with that signature, the gate reports a quarantined flake and passes.
  Any other failure in the same job still fails the gate.

`ci-gate` reads the list from the base commit, so a PR cannot quarantine its
own failure. An expired entry fails `CI: flaky quarantine` on every PR. The
nightly `Flaky job scan` opens or updates a `flaky` issue for every job that
failed and then passed on a rerun of the same commit.

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
| `linux-kvm (concurrent)` (Nightly: VM Space ready after boot) | 1 of 4 concurrent creates gets ghcr `Not authorized` on a public manifest; others miss the 10 s spacesd connect window | unknown: likely a token race on the shared registry client, plus 4 cold guests on a hosted runner | Spaces (CUA-3132) | quarantined: one retry, only when every failure matches those two signatures |
| `fmt, clippy, test (windows-latest)` (cua-spacesd): `volume_portmap::serves_over_tcp_and_udp` | `PermissionDenied` (os error 10013) from `Portmap::start(0)`, all retries | the runner excludes a UDP block and Windows hands out TCP ports in order, so 32 TCP-first picks all landed in it | Spaces (CUA-3098) | fixed: picks alternate TCP-first and UDP-first and keep refused ports bound |
| macOS hosted E2E TCC seed | sqlite `database is locked` while seeding TCC.db | tccd holds TCC.db; the seeder's sqlite3 calls had no busy timeout | cua-driver (CUA-3133) | fixed: 30 s busy timeout and `BEGIN IMMEDIATE` |
