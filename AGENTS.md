# Repository agent guidance

## Contributor authorship

Follow [Preserve Contributor Authorship](.github/CONTRIBUTING.md#preserve-contributor-authorship).

- A `Salvaged from #<pr>` source pull request must differ from the landing pull request.
- When adapting a contribution directly in its existing pull request, keep the
  contributor as the commit author and credit adapting authors with
  `Co-authored-by` trailers instead of citing the pull request as its own source.
- Use GitHub-linked or GitHub noreply email addresses for commit authors and
  coauthors so the contributor-attribution check can resolve each identity.
- Tell the source contributor where their work shipped.

Do not reimplement submitted work solely to remove its authorship history.

## Issue and pull request workflow

Follow the contribution and selection contract in
[`CONTRIBUTING.md`](.github/CONTRIBUTING.md#choose-where-work-starts),
[`MAINTAINERS.md`](docs/contributing/MAINTAINERS.md#select-and-start-work), the
GitHub issue forms, [`rfcs/README.md`](docs/rfcs/README.md), and
[`SECURITY.md`](.github/SECURITY.md). Keep those files canonical instead of duplicating
their field lists, polling ladder, or RFC lifecycle here.

Coding agents act as workers within that contract, not as a planning authority.
GitHub holds the durable record: the issue or RFC carries the problem and the
decision, and the pull request carries the execution. Local schedulers, queues,
and worktree tooling are private conveniences; a reviewer must not need them to
understand, reproduce, or continue the work.

- when the authenticated GitHub account has write access, create and push the
  work branch directly in the canonical repository. Do not default to a personal
  fork merely because a fork remote exists. Use a fork only when write access is
  unavailable or the maintainer explicitly requests one, and verify the pull
  request head owner before reporting it;
- avoid noisy periodic status comments; keep progress in the linked pull request
  description; and
- route suspected vulnerabilities through the private process in
  [`SECURITY.md`](.github/SECURITY.md), never a public issue, RFC, pull request, log, or
  screenshot.

### Polling for work

When asked what to work on, follow
[`MAINTAINERS.md`](docs/contributing/MAINTAINERS.md#pull-model) and the
[polling skill](.agents/skills/poll-github-work/SKILL.md).

## Repository layout

[`Development.md`](docs/contributing/Development.md) maps components to paths and toolchains;
[`TESTING.md`](docs/contributing/TESTING.md) maps them to test commands and CI workflows. Rules
that are easy to miss:

- `libs/cua` (cua SDK, `cua` CLI, `cua daemon`) is the only client library.
  `cua-sdk` is its only UniFFI export crate. Bindings, the Spaces manifest,
  and bundled skills are generated or synced and have `--check` drift gates.
- `libs/cua-spacesd` is server-only and runs inside the sandbox. Client
  code goes in `libs/cua`; the driver depends on `libs/cua`, never the reverse.
  The driver never injects input itself: all input goes through cua-driver.
- The `.proto` files in `libs/cua/proto` are the contract. Changes are
  additive and pass `buf breaking`.
- Sandboxes are daemon-agnostic: lifecycle and readiness must not assume
  cua-spacesd or any other in-guest service.
- Tests that start VMs, containers, or host input are opt-in
  (off by default) and clean up what they create.

## Before you push: `bin/ci`

Run `bin/ci` before every push, and `bin/ci --fix` when it reports a failure
it can repair (formatting, lockfiles, generated bindings and reference docs).
It runs the CI checks your diff against `origin/main` affects and prints the
exact fix command for each failure. `bin/ci --install-hook` runs it for you
before every `git push`.

- **Quick stage.** CI starts with a `quick` job that runs `bin/ci --quick`
  (formatting, lockfiles, headers, lint). The heavy jobs wait for it, so a
  failure there stops the run in about a minute.
- **Agent pull requests need a sign-off.** After a green `bin/ci` on a clean
  tree, push and run `bin/ci --signoff`. That sets the `local-ci` status on the
  head commit. `ci-gate` reports an agent PR without it (and blocks it once
  `REQUIRE_LOCAL_CI` is on): "Run bin/ci and bin/ci --signoff on this head
  commit." Sign off again after every new commit.

Reviewers of an agent PR check, before approving, that the `local-ci` status is
`success` on the current head commit (`gh api repos/trycua/cua/commits/<sha>/statuses`)
and that the PR's `bin/ci` checklist item is ticked. No status, no approval.

**Agents on shared hosts (the dev host that runs live VMs) use `bin/ci --quick`.**
It runs only the cheap checks (formatting, lockfiles, headers, lint) and builds
nothing. Heavy checks (cargo test, clippy, build, cross-target) go to CI or a
VM. A full `bin/ci` is disk-guarded: it skips heavy steps below
`CUA_CI_MIN_FREE_GB` (40) free, and stops cargo and exits 3 if the target dir
passes `CUA_CI_MAX_TARGET_GB` (15) or free disk falls under 25 GB. The target
dir is per run and deleted on exit unless you pass `--keep-target`.

A green `bin/ci` does not replace CI: the end-to-end, image and native Windows
lanes still run there. See [`CI.md`](docs/contributing/CI.md#run-the-checks-locally-first-binci).

## Cross-platform Cua Driver behavior

Treat user-visible Cua Driver behavior as a cross-platform contract. Implement
shared state, geometry, timing, and protocol semantics in the common crates
whenever possible, then keep each macOS, Windows, X11, and Wayland adapter thin.
Do not silently land a platform-specific cursor or interaction change as though
it were universal.

For every affected platform, add focused coverage and either verify the native
behavior or document a concrete operating-system or compositor limitation.
When a platform cannot support the same contract, return or publish that
limitation explicitly instead of substituting misleading behavior.

## Expensive end-to-end test timing

During implementation, use focused unit, contract, and platform smoke tests
plus the repository's ordinary pull request CI. Do not repeatedly run the full
representative desktop matrix against intermediate commits when the affected
code or test plan is still changing.

Run the complete cross-platform desktop E2E matrix once the implementation is
stable, on the exact candidate SHA immediately before the pull request is made
ready or merged. If that candidate changes afterward, rerun only the evidence
affected by the change; a product, harness, generated-contract, or environment
change normally requires recertification, while an instructions-only or other
demonstrably non-executable change does not require repeating unrelated desktop
rows. Record the tested SHA and account for any final diff explicitly.

After merge, run a short main-branch smoke and release-path verification. Repeat
the full matrix after merge only when the merge result materially differs from
the certified candidate or the smoke test exposes a regression.

## Canonical Cua Driver desktop E2E

Follow the [test harnesses guide](libs/cua-driver/docs/test-harnesses-guide.md#the-short-version)
and [CI runner guide](scripts/ci/README.md#desktop-runners) for canonical commands,
environment prerequisites, and evidence authority.

Do not assume that GitHub-hosted Windows runs in Session 0. Keep the hosted Linux
X11 gate, Nix source checks, and compositor-specific Wayland lanes separate.

## Cua Driver test ownership

These rules come from the #4094 test audit.

- Give each contract one owner test, at the strongest boundary that can
  observe it. Delete weaker duplicates instead of keeping parallel copies.
- Do not add test-only switches, environment hooks, or injectors to shipped
  code. Test through the public interface or a real seam.
- Plain `cargo test` must be hermetic: no real input, clipboard, window, or
  per-user host state. Use the testkit's isolated home. Clipboard round trips
  need an explicit `CUA_TEST_ALLOW_CLIPBOARD=1` opt-in.
- Put desktop-bound suites in `cua-driver-e2e` or mark them `#[ignore]`. A
  canonical runner must select every ignored test, or
  `libs/cua-driver/tests/manual-e2e-allowlist.txt` must list it with a reason;
  `.github/scripts/tests/test_cua_driver_e2e_inventory.py` enforces this.
- CI runs whole crates or test binaries, not hand-maintained module filters,
  so a new test module cannot be silently left out.
- A negative test asserts the exact refusal code or error, not just failure.
- When pruning or merging tests, mutation-check the keeper: break the
  behavior and confirm the remaining test fails.

## Pull request titles and component releases

Follow the [release-title rules](.github/CONTRIBUTING.md#agent-assisted-contributions)
and [Sandbox release rules](.github/CONTRIBUTING.md#sandbox-releases).
Do not use `no-release` to hide a user-visible change.

Before declaring a pull request ready or merging it, inspect its final changed
files and query its current GitHub title. Correct the title yourself when the
scope or release impact changed during implementation, and wait for
`CI: Release metadata` to pass. Do not leave title correction for a maintainer.

When you add notes to a Release Please PR body, put them below its version
heading (`## [x.y.z](...)`), never above it. Release Please reads the version
from that heading; with text above it, it tags every component with the PR's
version (#6350). `CI: Release metadata` and the release workflow both check this.

## Cua Spaces: no betas

Never tag a Cua Spaces beta (`cua-spaces-vX.Y.Z-beta.N`): the release workflow
refuses it unless the repository variable `ALLOW_SPACES_BETA` is `true`. For
pre-release testing, use the Nightly channel (Settings, Advanced, Nightly), or
run the **Nightly: Cua Spaces** workflow (`nightly-cua-spaces.yml`) to build
main now. Stable stays Release Please plus the canary gate. Details:
[docs/contributing/releasing-cua-spaces.md](docs/contributing/releasing-cua-spaces.md).

## Monorepo component release resolution

Follow [`docs/component-release-workflow.md`](docs/component-release-workflow.md)
for the normal Release Please preparation, publication, and verification path.

Cua is a monorepo with independent component release streams. Never use
GitHub's repository-wide "Latest" release badge, the `/releases/latest`
endpoint, or a generic "stable" designation to determine whether a component
has shipped or which version users receive.

Inspect the component's canonical installer and release workflow instead. For
Cua Driver, both the Unix and Windows installers normally use a
release-managed baked version. Their API fallback selects the highest semantic
version whose tag matches `cua-driver-rs-v*`. Both download assets from the
exact component tag; neither depends on GitHub's repository-wide "Latest"
release.

Nothing may depend on "Latest" at all (decided 5 Nov 2026). Installers,
update checks, the download page, docs and CI read an explicit per-product
source: a release-managed baked version, a release list filtered by the
product's tag prefix, or a fixed-tag rolling release that holds a feed or
manifest (`cua-install-latest`, `cua-spaces-latest`). Which release GitHub
marks Latest is cosmetic, for people browsing the repository.
`.github/scripts/tests/test_no_latest_consumers.py` fails on any new lookup.

Before making a release-status or installation-version claim:

- identify the component's tag prefix and canonical installer entry points;
- inspect version-override and baked-version precedence;
- verify that the component-tagged release and expected assets exist; and
- confirm which exact version the canonical installer currently resolves.

Describe a component as shipped when its own release artifacts exist and its
canonical distribution path resolves them. Do not add a separate
"promoted to GitHub Latest/stable" requirement unless that component's
distribution code explicitly uses one.
