# Known flaky checks

Rule: a flake is fixed at its root cause (race, timeout, port, shared state),
not with retries or raised limits. A check stays here only until its fix
merges. An entry without an owner is a bug in this file.

| Check | Symptom | Root cause | Owner | Status |
|---|---|---|---|---|
| `tag-safety-test` (CD: Image linux) | "tag not found" while the tag is listed | `crane ls \| grep -q` under `pipefail` is killed by SIGPIPE | images | fixed (PR 5663) |
| `jev-use` macOS native AppKit mock E2E | no `capture_id` | Cua Driver 0.37.0 needs macOS's one-time direct-capture approval; the runner had none | jev-use | fixed (PR 5665) |
| `jev-use` Linux mock E2E | `could not complete through A-owned authority` | unknown, intermittent | jev-use / cua-driver | open, needs root cause |
| `bindings drift` | cancelled at the timeout on a cold cache | cold release build of cua-sdk; the generator now builds the dev profile | SDK | fixed on staging, public follows |
