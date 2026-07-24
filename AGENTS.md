# Repository agent guidance

## Contributor authorship

Preserve contributor credit when external code or design ships in Cua.

- Merge the contributor's pull request when it can land directly.
- When moving a commit, use `git cherry-pick -x <sha>` so its author and source
  commit remain in history.
- When adapting material parts of a contribution in a new commit, add the
  contributor with a `Co-authored-by` trailer and write `Salvaged from #<pr>`
  in the commit or landing pull request body. The source pull request must be
  different from the landing pull request.
- When adapting a contribution directly in its existing pull request, keep the
  contributor as the commit author and credit adapting authors with
  `Co-authored-by` trailers instead of citing the pull request as its own source.
- Use GitHub-linked or GitHub noreply email addresses for commit authors and
  coauthors so the contributor-attribution check can resolve each identity.
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
