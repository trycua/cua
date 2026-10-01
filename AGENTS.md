# Repository agent guidance

## Contributor authorship

Follow [Preserve Contributor Authorship](CONTRIBUTING.md#preserve-contributor-authorship).

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
[`CONTRIBUTING.md`](CONTRIBUTING.md#choose-where-work-starts),
[`MAINTAINERS.md`](MAINTAINERS.md#select-and-start-work), the
GitHub issue forms, [`rfcs/README.md`](rfcs/README.md), and
[`SECURITY.md`](SECURITY.md). Keep those files canonical instead of duplicating
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
  [`SECURITY.md`](SECURITY.md), never a public issue, RFC, pull request, log, or
  screenshot.

### Polling for work

When asked what to work on, follow
[`MAINTAINERS.md`](MAINTAINERS.md#pull-model) and the
[polling skill](.agents/skills/poll-github-work/SKILL.md).

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

Follow the [release-title rules](CONTRIBUTING.md#agent-assisted-contributions)
and [Sandbox release rules](CONTRIBUTING.md#sandbox-releases).
Do not use `no-release` to hide a user-visible change.

Before declaring a pull request ready or merging it, inspect its final changed
files and query its current GitHub title. Correct the title yourself when the
scope or release impact changed during implementation, and wait for
`CI: Release metadata` to pass. Do not leave title correction for a maintainer.

## Monorepo component release resolution

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

Before making a release-status or installation-version claim:

- identify the component's tag prefix and canonical installer entry points;
- inspect version-override and baked-version precedence;
- verify that the component-tagged release and expected assets exist; and
- confirm which exact version the canonical installer currently resolves.

Describe a component as shipped when its own release artifacts exist and its
canonical distribution path resolves them. Do not add a separate
"promoted to GitHub Latest/stable" requirement unless that component's
distribution code explicitly uses one.
