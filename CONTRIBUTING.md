# Contributing to Cua

Thanks for contributing to Cua. The repository includes Python and TypeScript
SDKs, a Rust desktop driver, Swift virtualization tools, container images, and
public documentation. Start with the component that owns the behavior you want
to change.

## Choose Where Work Starts

| You want to                                                                                          | Start with                                                                                                                              |
| ---------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| Report reproducible incorrect behavior                                                               | The **Bug report** issue form                                                                                                           |
| Describe a user problem or product improvement                                                       | The **Feature request** issue form                                                                                                      |
| Change a public contract, compatibility policy, permission boundary, or cross-component architecture | The **Request for comments** issue form and [`rfcs/README.md`](rfcs/README.md)                                                          |
| Report a suspected vulnerability                                                                     | [GitHub private vulnerability reporting](https://github.com/trycua/cua/security/advisories/new), following [`SECURITY.md`](SECURITY.md) |
| Ask for contributor help                                                                             | The [Cua Discord community](https://discord.com/invite/mVnXXpdE85)                                                                      |

The issue tracker is an intake queue and historical record, not a promise that
every open item is scheduled or ready for implementation. An open issue means
the report was received. It does not mean anyone is working on it, and it does
not reserve the work.

Before substantial implementation:

1. Search open issues and pull requests for the same problem. If an active pull
   request already exists, contribute there or explain why your pull request
   supersedes it.
2. Comment on the issue with the scope you intend to implement and the
   acceptance evidence you will produce.
3. Open a draft pull request as soon as you have a branch and link it to the
   issue. The linked draft pull request is the visible claim marker that helps
   prevent duplicate work.

Selection must be visible in GitHub through an issue assignment, a maintainer
reply confirming the scope, or maintainer review of the linked draft pull
request. Small, self-contained fixes may go directly to a focused pull request
when the problem and evidence are clear; until reviewed, that work is an
unselected contribution. Work that needs an RFC waits for the recorded decision
in [`rfcs/README.md`](rfcs/README.md) before implementation begins.

Maintainers and coding agents choosing what to work on next should use the
read-only polling ladder in [`MAINTAINERS.md`](MAINTAINERS.md).

Once implementation starts, keep the issue or RFC as the problem and decision
record and the pull request as the current execution record. Link them in both
directions and keep the pull request description current as scope, validation,
or known gaps change. Use `Refs #123` for related work. Use `Fixes #123` only
when merging the pull request will fully resolve that issue.

## Report a Bug

Before opening the **Bug report** issue form, search the existing issue tracker.
Include:

- a concise description and reproducible steps;
- expected and actual behavior;
- Cua package or driver version;
- operating system, window system, and application when relevant;
- logs, structured errors, screenshots, or recordings that help reproduce it.

Do not include credentials or private application data.

## Propose a Change

In the **Feature request** issue form, describe the user problem and the
expected behavior before prescribing an implementation. Mention affected
platforms and existing workarounds when known.

Use the [RFC process](rfcs/README.md) before implementation when a proposal
changes a public SDK, CLI, MCP, protocol, compatibility, permission, or
cross-component architectural contract. Start with the **Request for comments**
GitHub issue form. Longer proposals and diagrams remain in this repository under
`rfcs/` and link back to the discussion issue.

## Submit Code

1. Read [`Development.md`](Development.md) and the guide next to the component.
2. Keep changes scoped to the component that owns the behavior.
3. Add or update tests that observe the public effect of the change.
4. Run the applicable commands in [`TESTING.md`](TESTING.md).
5. Run the formatters and linters owned by the changed component.
6. Open a focused pull request that explains behavior, validation, and known gaps.

### Agent-Assisted Contributions

Agent-assisted pull requests are welcome and are held to the same standard as
any other contribution. The person who opens the pull request is accountable
for it: read and understand the diff, be able to explain why each change is
present, and run the checks claimed in the description. Generated output or a
successful tool response is not validation evidence by itself.

Use a Conventional Commit title because the squash-merge title becomes the
release entry. `fix(cua-driver): preserve input while reconnecting` produces a
patch release, `feat(lume): add a VM readiness probe` produces a minor release,
and `feat(cua-driver)!: remove the legacy event endpoint` marks a breaking
change. `perf` and `revert` also produce releases.

Use `docs`, `test`, `ci`, `chore`, `build`, `refactor`, or `style` only when the
pull request has no user-facing release entry. A pull request that adds tests
while changing production behavior must be titled for the production change,
not the tests. If release-tracked Cua Driver or Lume files changed but the work
is intentionally non-releasing, add the `no-release` label. The
`CI: Release metadata` check enforces this contract before squash merge.

## Preserve Contributor Authorship

Keep the original author when external code or design ships in Cua. Merge the
contributor's pull request when possible. If you move their commit, use
`git cherry-pick -x <sha>` so the original author and source commit remain in
history.

If a maintainer adapts material parts of a contribution in a new commit,
include a `Co-authored-by` trailer with the contributor's GitHub no-reply email
and link the source pull request in the landing pull request. Use a line such as
`Salvaged from #123` so release automation can recover the source author.
Prefer a commit email linked to the contributor's GitHub account, especially a
GitHub no-reply address. This also applies to the pull request author's own
commits: GitHub may turn an unlinked commit author into a `Co-authored-by`
trailer during squash merge. If preserved authorship uses an email GitHub cannot
resolve, add the verified email-to-login entry to
`.github/release-attribution-config.json` in the same pull request. The
contributor-attribution check provides the exact `identityOverrides` JSON when
one explicitly referenced source pull request proves a unique mapping; it never
guesses from a name or email. Attribution preservation and resolvability must
land together. Preserve human coauthor trailers during rebases and squash
merges. Honor public credit opt-out requests and keep security-report
attribution private until the report can be disclosed.

Root pre-commit hooks are optional local helpers. Install them with:

```bash
uv sync --group dev
uv run pre-commit install
```

Mypy is configured but is not currently a pre-commit gate. Rust, TypeScript,
Swift, and documentation checks remain component-owned.

## Desktop Behavior Changes

cua-driver behavior must be verified through the canonical Rust harnesses. A
successful tool response alone is not evidence that an action reached the
application. Delivery tests should observe fixture state and attach focus,
z-order, cursor, leaked-input, capture, or refusal oracles as required.

Do not weaken a test to match the current driver. Add a capability, return an
exact structured refusal, or record the behavior as an explicit gap.

## Documentation

Public documentation lives under `docs/content/docs` and follows Diataxis. See
[`docs/README.md`](docs/README.md) before adding a page. Contributor-only plans,
journals, and implementation notes belong next to their component.

Documentation changes should pass generator drift, hygiene, internal links,
and the production Fumadocs build.

## Community

For design discussion and contributor help, join the
[Cua Discord community](https://discord.com/invite/mVnXXpdE85).
