# Self-hosted Mac runners for CI

Cua's macOS CI jobs that need a large machine can run on our own Apple-silicon Macs instead of GitHub's paid `macos-26-xlarge` runners. One repository variable controls it. Leave the variable unset and every job runs on `macos-26-xlarge`, as before.

## Turn it on or off

Set the repository variable `MAC_RUNNER_LABELS` (Settings, Secrets and variables, Actions, Variables) to a JSON value:

```json
["self-hosted","macOS","ARM64","cua-ci"]
```

Delete the variable to go back to GitHub-hosted `macos-26-xlarge`. No workflow change is needed either way.

## The fork-PR rule

`trycua/cua` is public. A pull request from a fork can change workflow files and run any code it likes, so it must never run on our Macs.

Every switched `runs-on` uses the self-hosted labels only when the event is one of:

- `push`
- `merge_group`
- `workflow_dispatch`
- `pull_request` where `github.event.pull_request.head.repo.full_name == github.repository` (same-repo branches, which only people with write access can create)

Every other event, including fork pull requests and `schedule`, falls back to `macos-26-xlarge`. The gate is in the `runs-on` expression itself, so it holds even when the variable is set.

Rules that go with it:

- Never use `pull_request_target` together with a checkout of pull-request code.
- Keep the runner group restricted to `trycua/cua` (below), and keep "Approval for running fork pull request workflows" on for outside contributors.
- Do not give these runners repository or deployment secrets beyond what the job already receives from the workflow.

## Register the runners

Use ephemeral runners: one job per runner, then the runner is removed and its workspace is wiped. A job can then never see the files, caches or processes of an earlier job.

1. Create a runner group in the organisation (Settings, Actions, Runner groups), for example `cua-ci-macos`. Set "Repository access" to selected repositories and choose only `trycua/cua`. Leave "Allow public repositories" on for that group, because the repository is public, and keep the repository list to that one entry.
2. Give each runner the labels `self-hosted`, `macOS`, `ARM64` and `cua-ci`. These match the example value above. Add nothing else to the group.
3. Register with just-in-time configuration, so there is no long-lived registration token on the Mac. A small supervisor loop on each Mac does:
   - ask GitHub for a JIT config with `POST /repos/trycua/cua/actions/runners/generate-jitconfig` (fields `name`, `runner_group_id`, `labels`, `work_folder`);
   - start `./run.sh --jitconfig <encoded config>`;
   - when the process exits (one job done), wipe the work folder and start the next one.
   The supervisor's GitHub credential should be a GitHub App installation token limited to "Self-hosted runners: read and write" on `trycua/cua`. Keep it out of the runner user's environment.
4. If JIT is not available for a Mac, register with `./config.sh --ephemeral` instead. The result is the same: one job, then the runner deregisters.

Run one runner per concurrent job you want. A Mac mini with 10 cores and 32 GB RAM handles one of these builds well. Two at once compete for the Rust and Swift builds and are slower than one each.

## Dedicated macOS user

- Create a standard (non-admin) macOS user, for example `ci-runner`, on every Mac. Do not use a personal account.
- The runner and its supervisor's child processes run as that user. The user is not in the `admin` group and has no `sudo`.
- Nothing from a personal profile (SSH keys, browser data, keychains with other credentials, `~/.aws`) lives in that account.
- Use a dedicated user keychain with no secrets. Release signing and notarization do not run on these runners.
- Turn off sleep and automatic software updates that reboot the Mac unattended, and let the user log in automatically only if a job needs a GUI session.

## Xcode and toolchains

Pin Xcode so a macOS or Xcode update cannot change a build overnight.

- Install the same Xcode version GitHub's `macos-26` image ships. Check the image's software list at the time of switching and write the version number in this file when you set the Mac up.
- Install it side by side with `xcodes`, then select it with `sudo xcode-select -s /Applications/Xcode_<version>.app` (done once by an admin, not by CI). Run `sudo xcodebuild -license accept` and `xcodebuild -runFirstLaunch` once.
- Check it in the supervisor before each job: `xcodebuild -version` must equal the pinned version, or the Mac stops taking jobs.
- Update Xcode on purpose: change one Mac, run CI on it, then roll to the others.

What the switched jobs need on the Mac:

| Need | Notes |
| --- | --- |
| Xcode and the Swift toolchain | `swift build` and `swift test` for the Swift packages and samples. Command line tools alone are not enough. |
| Rust | Installed per job by `dtolnay/rust-toolchain` (or `rustup`). Needs `rustup` and network access. |
| `protoc` | Installed per job by `.github/actions/setup-protoc`. |
| Node and pnpm | Installed per job by `actions/setup-node` for the Spaces bundle. |
| Homebrew | Only the tools the jobs call directly. Install them on the image, not at job time, because the runner user cannot use `sudo`. |
| Simulators | None of the switched jobs use iOS, watchOS or visionOS simulators. Do not install them. |
| Screen and Accessibility permissions | None of the switched jobs drive the GUI. |

Setting the Xcode and tool versions once on the Mac keeps job time down. Anything a job installs itself goes into the runner's work folder or tool cache and disappears with the ephemeral runner.

## Disk cleanup

Rust target directories and Swift build folders are large (tens of GB across a few runs).

- The supervisor deletes the runner work folder after every job.
- Before each job, if free space is under 100 GB, delete `~/Library/Developer/Xcode/DerivedData`, `~/Library/Caches/org.swift.swiftpm`, and any `target` folders outside the work folder. If it is still under 100 GB, the Mac stops taking jobs and alerts.
- Do not share a `CARGO_HOME` or `target` directory between jobs. Use the `actions/cache` and `rust-cache` steps in the workflows, so every job starts from a clean workspace.
- Check weekly with `df -h /` and `du -sh ~/Library/Developer`.

## Jobs switched

| Workflow | Job | Default label |
| --- | --- | --- |
| `ci-openkoalabot-example-samples.yml` | `swift` | `macos-26-xlarge` |
| `ci-infinite-canvas-swift.yml` | `swift` | `macos-26-xlarge` |
| `e2b-bindings.yml` | `swift` | `macos-26-xlarge` |
| `ci-cua-spaces-macos.yml` | `ffi`, `swift-tests`, `app-bundle` | `macos-26-xlarge` |
| `ci-cua-sdk.yml` | `rust` (macOS matrix entry) | `macos-26-xlarge` |
| `ci-cua-spaces.yml` | `bundle` (`macos-dmg` matrix entry) | `macos-26-xlarge` |

Jobs on the free standard `macos-26`, `macos-15` and `macos-latest` labels are not changed. Linux and Windows larger runners are not changed.

## Checking it works

1. With the variable unset, open a PR that touches one of the workflows above and confirm its Mac jobs still run on `macos-26-xlarge`.
2. Set `MAC_RUNNER_LABELS`. Re-run a same-repo PR and confirm the job log's "Runner name" shows a self-hosted runner.
3. Open a PR from a fork (or a throwaway fork of the repository) and confirm its Mac jobs run on `macos-26-xlarge` and never wait for a self-hosted runner.
