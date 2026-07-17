# Contributing to Cua

Thanks for contributing to Cua. The repository includes Python and TypeScript
SDKs, a Rust desktop driver, Swift virtualization tools, container images, and
public documentation. Start with the component that owns the behavior you want
to change.

## Report a Bug

Before opening an issue, search the existing issue tracker. Include:

- a concise description and reproducible steps;
- expected and actual behavior;
- Cua package or driver version;
- operating system, window system, and application when relevant;
- logs, structured errors, screenshots, or recordings that help reproduce it.

Do not include credentials or private application data.

## Propose a Change

For feature requests, describe the user problem and the expected behavior
before prescribing an implementation. Mention affected platforms and existing
workarounds when known.

## Submit Code

1. Read [`Development.md`](Development.md) and the guide next to the component.
2. Keep changes scoped to the component that owns the behavior.
3. Add or update tests that observe the public effect of the change.
4. Run the applicable commands in [`TESTING.md`](TESTING.md).
5. Run the formatters and linters owned by the changed component.
6. Open a focused pull request that explains behavior, validation, and known gaps.

Use a Conventional Commit title because the squash-merge title becomes the
release entry. `fix(driver): preserve input while reconnecting` produces a
patch release, `feat(lume): add a VM readiness probe` produces a minor release,
and `feat(driver)!: remove the legacy event endpoint` marks a breaking change.
Use `docs`, `test`, `ci`, or `chore` when the pull request has no user-facing
release entry.

## Preserve Contributor Authorship

Keep the original author when external code or design ships in Cua. Merge the
contributor's pull request when possible. If you move their commit, use
`git cherry-pick -x <sha>` so the original author and source commit remain in
history.

If a maintainer adapts material parts of a contribution in a new commit,
include a `Co-authored-by` trailer with the contributor's GitHub no-reply email
and link the source pull request in the landing pull request. Use a line such as
`Salvaged from #123` so release automation can recover the source author.
Preserve human coauthor trailers during rebases and squash merges. Honor public
credit opt-out requests and keep security-report attribution private until the
report can be disclosed.

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
