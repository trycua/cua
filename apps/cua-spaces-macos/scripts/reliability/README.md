# Launch reliability suite (Cua Spaces for macOS)

`suite.py` drives the runtime the SwiftUI app ships: its bundled `cua` (the
Cua Spaces build, `cua-spaces-cli`) and the `cua daemon` it runs. Each
scenario runs against a throwaway `CUA_HOME` and runs N times (default 3).
Every step has a time budget, and no step waits without a bound.

```sh
# The signed app's cua (copy the app out of the DMG; do not install it):
cua="$PWD/Cua Spaces.app/Contents/MacOS/cua"
# Or a local build: cargo build --release -p cua-spaces-cli (libs/cua)
python3 apps/cua-spaces-macos/scripts/reliability/suite.py --cua "$cua" \
  --mit-cua libs/cua/target/release/cua \
  --teleport-test "$(cargo test -p cua-spaces-ext --test e2e_teleport_app --no-run 2>&1 | grep -o '/.*e2e_teleport_app-[0-9a-f]*')" \
  --runs 3
```

`--scenarios` picks a subset:

- `fresh`
- `stale-socket`
- `stale-daemon` (`--other-cua` adds another bundle's `cua`)
- `crash`
- `two-clients`
- `restart`
- `offline`
- `lowdisk`
- `linux`
- `volume`
- `volume-macos`
- `macos-slim`
- `macos`
- `teleport`

Results are written to `<work>/results/<stamp>.json` and `.md`. The default
work directory is `~/projects/.cua-work/reliability`.

Host safety:

- Every process runs with its own `CUA_HOME`, the file credential store, no
  browser, telemetry off and a throwaway teleport home.
- The suite stops or kills only daemons that it started from its own homes.
- It deletes only the Lume VMs its homes recorded creating, and only the
  containers that carry its run prefix.
- At most one Space runs at a time. macOS VMs get 6 GiB.
- The teleport scenario uses a fixture Chrome bundle, a synthetic profile and
  a `FakeHost`. Nothing reads `/Applications`, a real profile or the Keychain.

The latest results are in the branch's report (PR notes).

## Cloud conformance (`cloud_suite.py`)

Spaces and sandboxes in your own cloud account (`--on aws|gcp|modal`), end to
end against the real cloud and relay.cua.ai. It is a sibling of `suite.py`
(and imports its helpers) because it needs the opposite home: one signed-in
test home that it never deletes (the relay account and device enrollment
live there), plus the cloud's own inventory before and after every run.

```bash
cloud_suite.py --cua ./target/debug/cua --on aws,gcp,modal --runs 3 \
    --home ~/projects/.cua-work/cloud-providers/home
```

- `space` scenario:
  - create, ready (spacesd and the guest doctor);
  - screenshot, exec, a file round trip;
  - a desktop stream and a window stream over the relay, and presence;
  - Cua Volume (with the Cua Spaces `cua` only);
  - stop and start (not on Modal);
  - delete, and an orphan check: nothing Cua tagged for the run is left, and
    every resource the account had before is unchanged.
- `bench` scenario: one `cua-bench-basic` task through Python cua-sandbox
  `on="<provider>"`, then the same orphan check.
- Each run records the steps' timings and an estimated cost (uptime times the
  provider's `usd_per_hour` for the image).

The suite deletes only what it created, by the names it chose, and never runs
`cua cloud sweep --delete`. At most one cloud machine per provider runs at a
time.
