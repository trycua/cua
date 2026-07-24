# Repository agent guidance

## Contributor authorship

Preserve contributor credit when external code or design ships in Cua.

- Merge the contributor's pull request when it can land directly.
- When moving a commit, use `git cherry-pick -x <sha>` so its author and source
  commit remain in history.
- When adapting material parts of a contribution in a new commit, add the
  contributor with a `Co-authored-by` trailer and write `Salvaged from #<pr>`
  in the commit or landing pull request body.
- Preserve known human coauthor trailers during rebases and squash merges.
- Link the source pull request and tell its author where the work shipped.
- Honor public credit opt-out requests. Keep security attribution private until
  disclosure is permitted.

Do not reimplement submitted work solely to remove its authorship history.

## Pull request titles and component releases

Pull requests are squash-merged, so the pull request title becomes the commit
subject on `main`. Release Please uses that subject to decide whether Cua Driver
or Lume receives a release. Treat the live pull request title as release
metadata, not as a cosmetic summary.

- Use `fix(cua-driver): ...` or `fix(lume): ...` for user-visible corrections
  that require a patch release.
- Use `feat(cua-driver): ...` or `feat(lume): ...` for new capabilities that
  require a minor release. Add `!` before `:` for a breaking release.
- `perf` and `revert` also produce releases. `test`, `docs`, `chore`, `ci`,
  `build`, `refactor`, and `style` do not.
- If release-tracked product files changed but the work is intentionally
  non-releasing, keep the accurate non-releasing type and add the `no-release`
  label. Do not use that label to hide a user-visible change.
- A pull request that mixes tests with production behavior must be titled for
  the production behavior. For example, browser fixes plus certification tests
  use `fix(cua-driver): ...`, not `test(cua-driver): ...`.

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
