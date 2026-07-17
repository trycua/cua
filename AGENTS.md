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
