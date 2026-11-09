<!--
Keep this description current as scope and evidence change. Do not include
credentials, private user data, sensitive screenshots, or vulnerability details.
Preserve external contributor authorship when their code or design ships.
While this pull request is a draft, use this description as the live record of
scope, progress, blockers, and evidence rather than posting periodic status comments.
-->

## Summary

- Problem this solves:
- What changed:

<!-- Describe observable behavior. Do not narrate the diff file by file. -->

## Related work

<!-- Use Fixes only when this PR fully resolves the issue. Otherwise use Refs. -->

Refs #

RFC (required for a public SDK, CLI, MCP, protocol, compatibility, permission,
or cross-component contract change):

## Compatibility and risk

- User-visible, API/CLI/MCP, migration, permission, or platform impact:
- Risk and rollback:

## Validation

- Focused tests and checks:
- Manual or platform evidence:
- Known gaps or CI still required:

## Screenshots (before/after)

<!--
UI changes only. Show before and after at desktop (1440x900) and mobile
(390x844), with demo data only: no real accounts, hostnames, chats or
credentials. Add a short recording when motion or timing matters. Upload the
images to GitHub and embed them here. Never commit PR-only images.
The "CI: PR screenshots" workflow links rendered images for the Spaces web UI.
To render more routes, add a line: Screenshot routes: /settings/connections
No UI change? Replace this section with: Screenshots: not needed - <reason>
-->

| | Before | After |
| --- | --- | --- |
| Desktop | | |
| Mobile | | |

## Contributor and release checks

- [ ] The PR is focused and the description matches the final diff.
- [ ] This change does not require an RFC, or the accepted RFC is linked above.
- [ ] Tests, documentation, and platform evidence are included or the gap is explained.
- [ ] The PR title is a Conventional Commit describing the production change.
- [ ] External contributor authorship is preserved, or no external contribution is included.
- [ ] If release-tracked files changed but this is intentionally non-releasing, the `no-release` label is applied.
- [ ] `bin/ci` is green and signed off (`bin/ci --signoff`)
