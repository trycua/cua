# Linux sandbox execution

Status: **release driver built; blocked staging Electron; not an end-to-end verification proof**.

The skill was retried from the repository root with the dependency-cache environment from `checks.out`:

```bash
export CARGO_HOME=/home/box/sand-data/onigiri/v01/cache/cargo
export RUSTUP_HOME=/home/box/.rustup
export npm_config_store_dir=/home/box/sand-data/onigiri/v01/cache/pnpm
export COREPACK_HOME=/home/box/sand-data/onigiri/v01/cache/corepack
export PNPM_HOME=/home/box/sand-data/onigiri/v01/cache/pnpm-home
export PATH=/home/box/.cargo/bin:$PATH
unset CARGO_TARGET_DIR CARGO_NET_OFFLINE
timeout 1260 .cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11
```

Xvfb, the private D-Bus session, openbox and picom launched. The WM probe passed and D-Bus activated `org.a11y.Bus`; its address was captured. Cargo compiled `cap-fs-ext` and the release driver successfully in 2m 42s at `libs/cua-driver/rust/target/release/cua-driver`. The previous claim that missing Cargo dependencies prevented the build was incorrect with these caches.

The next step failed with exit 1: `tests/fixtures/build/linux.sh --only electron` invoked `apps/cross-platform/electron/build.sh`, which printed `[INSTALL] npm install (first run)...` and exited during `npm install --silent`. To expose the suppressed error, the same Electron directory was checked with `timeout 120 npm install --loglevel verbose` under the same npm cache environment. That command also exited 1, reporting repeated package-download DNS failures, for example:

```text
npm http fetch GET https://registry.npmjs.org/@types/node/-/node-22.20.1.tgz attempt 3 failed with EAI_AGAIN
npm error Exit handler never called!
npm error This is an error with npm itself. Please report this error at:
npm error   <https://github.com/npm/cli/issues>
npm error Log files were not written due to an error writing to the directory: /home/box/.npm/_logs
```

The wrapper completed cleanup and printed:

```text
Evidence retained after cleanup: /home/box/sand-data/onigiri/cua/.git/onigiri/lanes/task-mvjsffom/artifacts/cua-driver/verify/run-hngSON71
```

A post-cleanup file listing confirmed these retained files:

```text
artifacts/cua-driver/verify/run-hngSON71/
  at-spi-address.txt
  cases.jsonl
  environment.jsonl
  launch.log
  openbox.log
  picom.log
  results.jsonl
```

The `recordings/` directory and all three result JSONL files are empty. Doctor and preflight were not reached: there is no `doctor.json`, `version.txt`, `health_report.json`, `check_permissions.json` or `daemon.json`. Neither background AX click cell ran; there are no transcripts, sentinel/native observations, fixture-state snapshots, screenshots, videos or delivered summary rows. Desktop setup and a successful build are not substitutes for those artifacts. Retry once Electron dependencies can be staged; this attempt does not prove delivery or daemon/fixture teardown.

No prebuilt driver, forged fixture result or test-only target setter was substituted. The documented Linux stand-ins remain Xvfb, a real fixture app, disposable state and testkit authorization bypass; the fixture never launched in this attempt. macOS, Windows and Wayland remain unverified. This round changes only the execution documentation, not production code or tests.
