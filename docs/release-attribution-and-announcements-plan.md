# Release attribution and announcements plan

- **Status:** Implemented locally; live fixture and rollout gates remain
- **Initial products:** Cua Driver and Lume
- **Audience:** Engineering, developer relations, and release owners

## Summary

We want every Cua Driver and Lume release to publish accurate change notes and credit the people who contributed. The same release data should later produce announcement drafts for X and other channels.

The proposed system uses Release Please as its release foundation. Conventional squash-merge titles provide the change type and user-facing summary. Release Please maintains independent release pull requests, versions, changelogs, tags, and GitHub releases for Cua Driver and Lume. A small Cua-owned step checks contributor credit and writes a machine-readable manifest for release visuals and social drafts.

We will automate facts and formatting first. A release owner will still review editorial summaries, visuals, and social posts before publication.

### Audit outcome

The independent Fable audit returned **REVISE** with high confidence. The overall architecture is sound, but the first draft assumed that Release Please's Rust strategy could update Cua Driver's virtual Cargo workspace and that the existing CD jobs could safely write to a Release Please draft by tag. Neither assumption is safe for this repository without changes.

This revision makes the initial design explicit:

- Treat the Cua Driver release as one product that contains nine Cargo crates.
- Use Release Please's `simple` strategy for Cua Driver, backed by a small `rust/VERSION` file and a targeted update of `[workspace.package].version`.
- Regenerate and commit Driver's inherited workspace versions in `Cargo.lock`
  with `cargo update -p cua-driver --precise <version>`.
- Pin the Release Please action and test the draft/tag behavior before cutover.
- Address draft releases by release ID while artifacts are being assembled.
- Give one finalizer job sole ownership of the release body and draft-to-published transition.
- Remove every legacy Cua Driver/Lume release trigger during cutover, including the `bump2version` menu entries, label-driven dispatch, reminder, and digest mappings.

### Implementation status (July 16, 2026)

The core repository now contains the proposed Release Please configuration,
version synchronizer, PR-first attribution collector, manifest schema, release
body/card/social renderers, draft finalizer, contributor policy, and Driver/Lume
workflow cutover. The old Driver and Lume bump configs and legacy route mappings
have been removed. The remaining generic bump workflow rejects existing tags
instead of deleting them.

Local verification currently includes:

- 48 passing release-script and wiring tests;
- validation against the pinned Release Please 17.3.0 config and manifest schemas;
- direct Release Please updater tests for all configured TOML, Python, shell,
  PowerShell, and Swift version sources;
- actionlint and YAML parsing for the changed workflows;
- locked Cargo metadata with all nine Driver workspace packages at `0.8.3`;
- an implementation review and re-audit by Claude Code Fable. The re-audit
  returned **PASS WITH FOLLOW-UPS**. Its findings about AI coauthors, linked
  Release Please changelog headings, and GitHub `[bot]` no-reply identities now
  have regression coverage; the remaining follow-ups are part of the branch
  fixture and rollout gates below.

The system is not ready to publish from `main` until the team completes the
branch fixture matrix below and makes `CI: Release metadata` a required check
for Driver and Lume release pull requests. That required check prevents a
Release Please branch refresh from dropping the generated Cargo.lock sync
commit before merge. The team must also review repository merge settings so
product pull requests use squash merge titles as release input.

## Why this work is needed

Cua Driver and Lume currently generate GitHub release notes from a path-filtered `git log`. The workflow formats the git author name as a GitHub mention and limits the output to 50 commits.

This causes several problems:

- A git author name is often different from the contributor's GitHub handle.
- Rebase, squash, and salvage workflows can hide the original pull request author.
- Coauthors and issue reporters do not receive credit.
- A 50-commit limit can silently omit changes.
- Commit subjects vary in quality and often describe implementation work instead of user impact.
- The release body is Markdown only, so later systems must parse prose to create announcements.
- Cua Driver and Lume duplicate similar release-note shell code.

The version bump workflow can also delete and recreate remote tags. Published release tags should be immutable. This safety fix should land before the new release-note pipeline.

## What we learned from Hermes and OpenClaw

### Hermes

Hermes generates release notes from commits between tags. Its release script categorizes conventional commits, resolves git authors, reads `Co-authored-by` trailers, prints a contributor section, supports a dry run, and creates a GitHub release.

Hermes also states that contributor authorship should survive when maintainers salvage external work. That policy is worth adopting.

The main cost is identity maintenance. Hermes has a large email-to-GitHub-handle map because git metadata alone cannot consistently identify GitHub contributors. We should avoid making such a map the core of our system.

Hermes occasionally adds a custom release infographic. The `v2026.6.5` Surface Release includes a 1024 by 1024 blueprint-style card with the main features and release statistics. This appears to be an editorial choice for a large release rather than a step used for every release.

### OpenClaw

OpenClaw builds a pull-request-first release manifest from git history and GitHub data. It then checks the human-written release summary against that manifest.

Its process separates two jobs:

1. The generated contribution record proves which pull requests, issues, and contributors shipped.
2. The editorial sections group that work by user impact.

OpenClaw validates missing pull requests, missing human attribution, invalid issue references, bot credit, duplicate records, and release-body drift. It also renders GitHub release notes from a tag-pinned changelog section.

OpenClaw's full process is larger than we need for two products. We should adopt its PR-first source of truth and validation rules without copying its complete release system.

The OpenClaw checkout was not available under `/Users/administrator/repo` during the local audit. The claims above come from the linked upstream files and must be rechecked against a pinned upstream revision before implementation. The Hermes claims were verified against the local `hermes-agent` checkout.

## Goals

- Credit external pull request authors in every patch, minor, and major release.
- Preserve known human coauthor credit.
- Credit issue reporters when their issue directly led to a shipped fix.
- Use verified GitHub handles instead of names inferred from git metadata.
- Produce useful, product-specific notes for Cua Driver and Lume.
- Keep each product's version and release history independent.
- Generate the GitHub body, changelog entry, contribution record, and release manifest from the same data.
- Support a reviewed infographic for major releases and selected feature launches.
- Produce structured data that can later generate X post drafts.
- Make release generation deterministic and testable.

## Non-goals for the first rollout

- Migrating every package in the monorepo.
- Automatically posting to X.
- Generating final editorial prose with no review.
- Backfilling perfect attribution for every historical release.
- Building a general marketing automation service in the cloud repository.
- Copying OpenClaw's full release evidence and multi-platform approval system.

## Proposed release model

### Tool choice

Common release systems solve different parts of this problem:

| System | Common use | Fit for Cua Driver and Lume |
| --- | --- | --- |
| GitHub generated release notes | List merged pull requests, contributors, and a compare link; group entries with labels | Useful as a fallback and reference, but it does not own component versions and its release range is repository-wide rather than path-scoped for each product. |
| Changesets | Collect per-pull-request change files and version packages in JavaScript monorepos | Its change-file model is good, but its package and publish flow centers on package-manager workspaces. Adapting it to a Cargo workspace and Swift executable would add Cua-owned glue. |
| dist (formerly cargo-dist) | Build and announce Rust applications from package-aware tags; emit a distribution manifest | A possible later improvement for Cua Driver packaging. It does not manage Lume's Swift version or a shared two-component release PR flow. |
| Release Please | Manage path-scoped components, versions, changelogs, release pull requests, tags, and GitHub releases | Best match. It supports Rust workspaces, a generic version-file strategy for Lume, independent component tags, and monorepo manifest mode. |

Hermes and OpenClaw both built custom systems because they need release behavior beyond these tools. Cua should begin with Release Please and add code only for attribution checks, the social manifest, and visual generation.

### Standard release foundation: Release Please

Use Release Please in manifest mode instead of creating a Cua-specific product catalog. Manifest mode is Release Please's standard monorepo configuration. It defines components by repository path, tracks each component's released version, creates release pull requests, updates version and changelog files, creates component-prefixed tags, and creates GitHub releases.

Release Please fits the two initial products:

- Its `simple` strategy can manage Cua Driver as one product while a targeted TOML updater changes `[workspace.package].version`.
- A release-PR sync step can run the same `cargo update --workspace` operation the current bump workflow uses, keeping every inherited workspace-member version in `Cargo.lock` aligned.
- Its `simple` strategy can use Lume's existing `VERSION` file and update `CHANGELOG.md`.
- Its generic extra-file updater can keep `libs/lume/src/Main.swift` in sync after we add an `x-release-please-version` annotation to the version line.
- Manifest paths and exclusions isolate the commits considered for Cua Driver and Lume. The Cua Driver scope includes the Rust workspace, shared installer scripts, and Wayland helper, while excluding its separately released Python package, internal docs, and test fixtures.
- Component-prefixed tags preserve `cua-driver-rs-vX.Y.Z` and `lume-vX.Y.Z`.
- Separate release pull requests preserve independent release timing.

The built-in `rust` strategy is not the initial choice because `libs/cua-driver/rust/Cargo.toml` is a virtual workspace: it has `[workspace.package]` but no root `[package]`, and all nine members use `version.workspace = true`. The current Release Please Rust updater attempts to update a root `[package].version` and rejects a virtual root. Its Cargo workspace plugin also assumes individually versioned package manifests, which is not Cua Driver's model.

Proposed configuration:

```json
{
  "separate-pull-requests": true,
  "include-component-in-tag": true,
  "include-v-in-tag": true,
  "tag-separator": "-",
  "draft": true,
  "force-tag-creation": true,
  "packages": {
    "libs/cua-driver": {
      "release-type": "simple",
      "component": "cua-driver-rs",
      "package-name": "cua-driver-rs",
      "version-file": "rust/VERSION",
      "changelog-path": "rust/CHANGELOG.md",
      "exclude-paths": [
        "libs/cua-driver/docs",
        "libs/cua-driver/python",
        "libs/cua-driver/tests"
      ],
      "extra-files": [
        {
          "type": "toml",
          "path": "rust/Cargo.toml",
          "jsonpath": "$.workspace.package.version"
        },
        {
          "type": "toml",
          "path": "python/pyproject.toml",
          "jsonpath": "$.project.version"
        },
        {
          "type": "generic",
          "path": "python/src/cua_driver/__init__.py"
        },
        {
          "type": "generic",
          "path": "scripts/_install-rust.sh"
        },
        {
          "type": "generic",
          "path": "scripts/install.ps1"
        }
      ]
    },
    "libs/lume": {
      "release-type": "simple",
      "component": "lume",
      "package-name": "lume",
      "version-file": "VERSION",
      "changelog-path": "CHANGELOG.md",
      "extra-files": [
        {
          "type": "generic",
          "path": "src/Main.swift"
        },
        {
          "type": "generic",
          "path": "scripts/install.sh"
        }
      ]
    }
  }
}
```

Seed Release Please's version manifest from the current product versions:

```json
{
  "libs/cua-driver": "0.8.3",
  "libs/lume": "0.3.16"
}
```

Add `libs/cua-driver/rust/VERSION` during implementation and seed it to the same value as `[workspace.package].version`. A release-PR workflow runs `cargo update -p cua-driver --precise <version>` after Release Please changes those two files. Cargo updates all nine inherited workspace-member versions without resolving unrelated dependencies. The workflow commits the resulting `Cargo.lock` change to the release pull request with the release GitHub App and fails if the version file, Cargo manifest, or lockfile disagree.

The exact configuration must be tested against a branch before adoption. In particular, the test must prove that the existing tags are detected, the Cua Driver exclusion paths behave as intended, an installer-only change enters the driver history, the Rust version files and lockfile update together, Lume's two version files stay equal, and tags created with the Cua release GitHub App token trigger the existing CD workflows.

Pin `googleapis/release-please-action` by commit SHA and record the embedded Release Please version. Do not point `$schema` at the moving `main` branch: the current schema and runtime have drifted on keys such as `component`, `package-name`, and `include-commit-authors`. Either use the schema from the exact embedded Release Please tag after validating it or omit `$schema` and validate the config with that pinned CLI/action in CI.

Use Release Please's default changelog renderer for component releases. Its optional `github` changelog renderer asks GitHub for repository-wide generated notes and does not use Release Please's path-filtered commit list. That would mix unrelated monorepo changes into Cua Driver or Lume notes.

Release Please documents `force-tag-creation` as eager creation of a tag ref when `draft` is enabled; it does not authorize overwriting or moving an existing tag. The branch fixture must still prove the complete integration: Release Please creates the component tag and draft release, the App-token-created tag starts the existing CD workflow, and the draft remains unpublished until the finalizer succeeds. If the pinned version does not satisfy that sequence, do not cut over; use a separate explicit tag-and-draft orchestration step instead.

Release Please should replace `bump2version` for these two components after the branch test passes. Other packages remain on the existing workflow.

### Pull request release input

Use Conventional Commit syntax in squash-merge titles instead of adding a custom change-fragment format:

```text
fix(driver): preserve keyboard input while the driver reconnects
feat(lume): add a VM readiness probe
feat(driver)!: remove the deprecated event endpoint
```

Release Please maps `fix` to a patch, `feat` to a minor, and `!` or a `BREAKING CHANGE` footer to a major. It also supports multiple release entries in one squash commit and a `BEGIN_COMMIT_OVERRIDE` block in the merged pull request body when the original summary needs correction.

Pull requests with no user-facing release entry should use a non-releasing type such as `chore`, `test`, `docs`, or `ci`. We can keep `release-note:none` as a review signal, but Release Please does not need it as release input.

A pull request that changes both products is included in both path-scoped component histories. Release Please scopes work at the commit level, so the same conventional entry and SemVer effect apply to both components. Multiple conventional entries in one squash commit would appear in both changelogs. If the products need different summaries or bump levels, split the work into product-specific pull requests before merge. This is the main limitation to test during the pilot.

### Contributor identity and roles

The collector should use the GitHub pull request as the main identity source. It should record:

- Pull request author
- Human coauthors with known GitHub handles
- Reporter of an issue closed by the pull request
- Pull request number and linked issue numbers
- Whether each contributor is external or a maintainer
- Whether this is the person's first credited release for that product, when the system can prove it

Each person can have one or more roles: `author`, `coauthor`, or `reporter`.

The repository should have small configuration lists for known bots, automation accounts, and internal handles. Manual identity overrides should cover exceptional historical or salvaged work only.

### Authorship preservation policy

Cua should adopt this repository policy:

> Contributor credit is preserved. When external code or design is incorporated into Cua, preserve the original git author or record the contributor as a coauthor, and link the source pull request. Do not reimplement submitted work solely to remove its authorship history.

Apply the policy as follows:

- Merge the contributor's pull request when it can land directly.
- When moving an external commit to another branch or pull request, use `git cherry-pick -x <sha>`. This preserves the original `Author` field and records the source commit.
- When a maintainer adapts an external commit but keeps most of its implementation, preserve the contributor as the commit author. Add the maintainer as a coauthor when that reflects the work.
- When a maintainer uses a material part of an external implementation or design in a larger rewrite, add a standard `Co-authored-by: Name <email>` trailer for the contributor.
- Link the original pull request in the landing pull request and commit body. Close the original pull request with a link to the commit or pull request that shipped the work.
- Preserve all known human `Co-authored-by` trailers when rebasing, squashing, or moving the work.
- Credit each person whose code or design materially shaped the shipped change. A bug report without contributed implementation uses the `reporter` role instead of git authorship.
- Respect a contributor's request to omit public credit. Security reports may need private attribution until disclosure.

Examples:

```text
# A contributor commit can land with little or no rewriting.
git cherry-pick -x <contributor-sha>
```

```text
# A maintainer commit incorporates material work from an external PR.
fix(driver): preserve input across reconnects

Adapted from #2345.

Co-authored-by: Alice Example <alice@users.noreply.github.com>
```

The attribution checker should resolve the original pull request reference, author, and coauthors before release. If a landing commit refers to an external source pull request, the corresponding contributor must appear in the release manifest unless the person opted out.

Once the team approves this plan, copy the policy into `CONTRIBUTING.md` and add a repository-level `AGENTS.md` so both humans and coding agents follow the same rule.

### Release manifest

The attribution step writes a manifest after Release Please creates the tag:

```json
{
  "schemaVersion": 1,
  "product": "cua-driver-rs",
  "displayName": "Cua Driver",
  "version": "0.8.2",
  "bump": "patch",
  "tag": "cua-driver-rs-v0.8.2",
  "sha": "<release-sha>",
  "previousTag": "cua-driver-rs-v0.8.1",
  "compareUrl": "https://github.com/trycua/cua/compare/cua-driver-rs-v0.8.1...cua-driver-rs-v0.8.2",
  "changes": [
    {
      "type": "fix",
      "summary": "Preserve keyboard input while the driver reconnects.",
      "pr": 2345,
      "issues": [2321],
      "contributors": [
        { "login": "alice", "role": "author", "external": true },
        { "login": "bob", "role": "reporter", "external": true }
      ]
    }
  ],
  "contributors": [
    { "login": "alice", "roles": ["author"], "external": true },
    { "login": "bob", "roles": ["reporter"], "external": true }
  ]
}
```

The merged Release Please pull request contains the version and product changelog update, so the tag pins both. After tagging, the attribution step writes the manifest from the exact tag range and release SHA, then attaches it to the GitHub release. The manifest must record the tag and SHA so downstream consumers can reject data from another release.

The release asset remains mutable because a repository writer can replace it or edit the release body. Binding it to the tag and SHA and publishing its checksum makes changes detectable. The first rollout should publish that checksum in the release workflow summary and verify it before publication. A later hardening step can anchor the checksum in an artifact attestation or another append-only record.

Suggested paths and assets:

```text
libs/cua-driver/rust/CHANGELOG.md
libs/lume/CHANGELOG.md
GitHub release asset: release-manifest.json
```

### GitHub release format

Every release should contain:

```markdown
# Cua Driver 0.8.2

<optional release visual>

## Summary

One or two sentences about the user outcome.

## Changes

- Preserve keyboard input while the driver reconnects. ([#2345](...)) Thanks @alice; reported by @bob.

## Contributors

Thanks to @alice and @bob for contributing to this release.

## Full changelog

[cua-driver-rs-v0.8.1...cua-driver-rs-v0.8.2](...)
```

Major and minor releases may split changes into `Highlights`, `Features`, and `Fixes`. Patch releases can use `Fixes` and `Other changes`. Every release gets a contributor section. When there are no external contributors, the section should state that the release contains maintainer changes only.

## Worked examples

The pull request numbers and contributor handles below are illustrative.

### Example 1: Cua Driver patch from an external contributor

An external contributor fixes lost keyboard events during a driver reconnect.

#### Contributor pull request

```text
Title: fix(driver): preserve keyboard input while the driver reconnects
Author: @alice
Files: libs/cua-driver/rust/**
Closes: #2321, reported by @bob
```

The pull request is squash-merged. The resulting main-branch commit retains the conventional title and GitHub association with pull request `#2345`.

#### Release Please pull request

Release Please opens or updates:

```text
chore(main): release cua-driver-rs v0.8.2
```

The release pull request changes:

```text
.release-please-manifest.json             0.8.1 -> 0.8.2
libs/cua-driver/rust/VERSION              0.8.1 -> 0.8.2
libs/cua-driver/rust/Cargo.toml           0.8.1 -> 0.8.2
libs/cua-driver/rust/Cargo.lock           regenerated by cargo update --workspace
libs/cua-driver/rust/CHANGELOG.md         adds the 0.8.2 section
```

Its generated changelog section will resemble:

```markdown
## [0.8.2](https://github.com/trycua/cua/compare/cua-driver-rs-v0.8.1...cua-driver-rs-v0.8.2)

### Bug Fixes

* **driver:** preserve keyboard input while the driver reconnects ([#2345](https://github.com/trycua/cua/pull/2345))
```

Release Please owns the typed summary and pull request link. It does not prove that a git author name is a verified GitHub handle. The Cua attribution checker adds the verified `@alice` and reporter credit to the final body and manifest from GitHub pull request and issue data.

#### Merge and publication

Merging the release pull request causes Release Please to create:

```text
Tag: cua-driver-rs-v0.8.2
GitHub release: draft
Target SHA: the merged release pull request commit
```

The Cua Driver CD workflow starts from the tag, builds the platform artifacts, looks up the matching draft by release ID, and uploads assets without changing its body or publication state. The attribution check confirms `@alice` as the pull request author and `@bob` as the linked issue reporter.

#### Final GitHub release body

```markdown
# Cua Driver 0.8.2

## Summary

This patch preserves keyboard input when the driver reconnects to a running session.

## Fixes

- Preserve keyboard input while the driver reconnects. ([#2345](https://github.com/trycua/cua/pull/2345)) Thanks @alice; reported by @bob in [#2321](https://github.com/trycua/cua/issues/2321).

## Contributors

Thanks to @alice and @bob for contributing to this release.

## Full changelog

[cua-driver-rs-v0.8.1...cua-driver-rs-v0.8.2](https://github.com/trycua/cua/compare/cua-driver-rs-v0.8.1...cua-driver-rs-v0.8.2)
```

#### Attached release manifest

```json
{
  "schemaVersion": 1,
  "product": "cua-driver-rs",
  "version": "0.8.2",
  "tag": "cua-driver-rs-v0.8.2",
  "sha": "<release-sha>",
  "changes": [
    {
      "type": "fix",
      "summary": "Preserve keyboard input while the driver reconnects.",
      "pr": 2345,
      "issues": [2321],
      "contributors": [
        { "login": "alice", "role": "author" },
        { "login": "bob", "role": "reporter" }
      ]
    }
  ]
}
```

#### Generated X draft

```text
Cua Driver 0.8.2 is out.

This patch preserves keyboard input when the driver reconnects to a running session.

Thanks @alice and @bob for the contribution and bug report.

Release notes: <release-url>
```

### Example 2: Lume minor release with a release card

A contributor adds resumable VM snapshots. The feature is large enough to warrant a release card.

#### Contributor pull request

```text
Title: feat(lume): add resumable VM snapshots
Author: @carol
Files: libs/lume/**
Label: release-visual
```

Release Please opens:

```text
chore(main): release lume v0.4.0
```

The release pull request changes:

```text
.release-please-manifest.json     0.3.16 -> 0.4.0
libs/lume/VERSION                0.3.16 -> 0.4.0
libs/lume/src/Main.swift         0.3.16 -> 0.4.0
libs/lume/CHANGELOG.md           adds the 0.4.0 section
```

The final release body could read:

```markdown
# Lume 0.4.0

![Lume 0.4.0 release card](<release-card-url>)

## Summary

Lume can now capture a running VM and resume it from the same disk and machine state.

## Features

- Add commands to create, list, restore, and delete VM snapshots. ([#2410](https://github.com/trycua/cua/pull/2410)) Thanks @carol.

## Fixes

- Prevent snapshot restore from reusing a stale guest-agent connection. ([#2417](https://github.com/trycua/cua/pull/2417)) Thanks @dan.

## Contributors

Thanks to @carol and @dan for contributing to Lume 0.4.0.

## Full changelog

[lume-v0.3.16...lume-v0.4.0](https://github.com/trycua/cua/compare/lume-v0.3.16...lume-v0.4.0)
```

The deterministic release-card template could render this layout:

```text
+------------------------------------------------------+
| LUME 0.4.0                         SNAPSHOT RELEASE  |
|                                                      |
|        [ VM ] ----------> [ SAVED SNAPSHOT ]         |
|          ^                         |                 |
|          +--------- RESTORE -------+                 |
|                                                      |
| Create and resume snapshots                          |
| Safer guest-agent reconnection                       |
|                                                      |
| 2 pull requests | 2 contributors | trycua/cua       |
+------------------------------------------------------+
```

The workflow renders the final 1600 by 1600 PNG from the manifest, uploads it to the draft release, inserts the asset URL into the body, and waits for approval before publication.

An X draft could read:

```text
Lume 0.4.0 adds resumable VM snapshots.

Capture a running VM, restore it later, and keep the same disk and machine state.

Thanks @carol and @dan for the work.

Release notes: <release-url>
```

### Example 3: One pull request touches both products

Suppose one pull request changes readiness handling in both Cua Driver and Lume:

```text
Title: fix: wait for guest readiness before accepting commands
Files:
  libs/cua-driver/rust/**
  libs/lume/**
```

Release Please sees the squash commit in both component paths. It proposes:

```text
Cua Driver: 0.8.1 -> 0.8.2
Lume:       0.3.16 -> 0.3.17
```

Both changelogs receive the same fix summary. The team may merge either release pull request first because they remain separate.

If the Cua Driver change is a new feature while the Lume change is a small compatibility fix, one squash commit cannot express a minor bump for one component and a patch bump for the other. Split the work into two pull requests:

```text
feat(driver): expose guest readiness state
fix(lume): wait for the driver readiness state
```

This keeps Release Please's standard path model accurate and avoids a custom per-product change format.

## Release visuals

### When to create one

- Major release: required
- Minor release with a release-defining feature: recommended
- Patch release: omitted unless the patch has an important security, compatibility, or data-safety story
- Feature launch between releases: allowed when it points to a shipped version

The release owner makes the final call. A `release-visual` label or manifest flag can request one during release preparation.

### Visual content

A release card should include:

- Product name and version
- A short release title
- Three to five user-facing changes
- A product diagram, UI crop, or simple illustration
- Contributor, pull request, or issue counts when those numbers add context
- Cua branding and accessible alt text

Use a square master image so the same asset works on GitHub and X. A 1600 by 1600 PNG gives enough resolution; the renderer can also emit a smaller preview.

### Generation method

The first version should use a deterministic SVG or HTML template. The release manifest supplies all text and numbers. Product-specific templates control colors, icons, and layout. This avoids misspelled product names, invented metrics, and distorted contributor handles.

An image model may later create a background illustration or feature art. The template should add all text after image generation. A release owner must review the final image for accuracy, legibility, and brand fit.

### Publishing the visual

The workflow can:

1. Render the image during release preparation.
2. Save it as a workflow artifact for review.
3. Create or update a draft GitHub release.
4. Upload `release-card.png` as a release asset.
5. Embed the stable asset URL in the release body.
6. Publish the release after the release owner approves the notes and image.

The X draft should reference the same asset rather than generating a second image.

## Social announcement output

The first social integration should generate a draft artifact. It should not post directly.

The renderer can produce:

- One short X post for patch releases
- A lead post and optional thread for feature-heavy minor or major releases
- A plain-text contributor list
- The release URL and visual asset URL
- Suggested alt text

Example draft:

```text
Cua Driver 0.8.2 is out.

This patch preserves keyboard input during driver reconnects.

Thanks @alice and @bob for the contributions.

Release notes: <url>
```

The draft must fit the platform's current character limits after links and contributor handles are inserted. Platform rules can change, so the posting integration should check them when we implement it.

The cloud repository should consume the release manifest only after the core workflow is stable. Attribution discovery and identity resolution should remain in the core repository, where the pull requests, tags, and changelogs live.

## Release workflow

### Pull request time

1. Detect changes under the two product paths.
2. Require a Conventional Commit-style pull request title.
3. Check that the type produces the intended SemVer effect.
4. Enforce squash merge for these product paths so the pull request title becomes the main-branch commit title.
5. Use `BEGIN_COMMIT_OVERRIDE` when one pull request needs several changelog entries or a corrected summary.
6. Preview the future changelog entry in CI.

### Release preparation

1. Run Release Please on `main` with the Cua release GitHub App token.
2. Let Release Please maintain one release pull request per component.
3. Review the proposed version, changelog entries, pull requests, and tag name.
4. Correct a merged pull request body with `BEGIN_COMMIT_OVERRIDE` if needed, then rerun Release Please.
5. Merge only the component release pull request that is ready to ship.
6. Let Release Please tag that merge commit and create the GitHub release.
7. Build the attribution manifest from that exact tag, SHA, component path, and GitHub pull request data.
8. Fail publication if an eligible human contributor is missing or a bot is thanked.
9. Render the optional visual and social draft from the verified manifest.

### Publication

1. Let the existing tag-triggered CD workflow build and test the exact tagged commit.
2. List GitHub releases, include drafts, and match the unique draft by `tag_name`; retain its release ID and upload URL.
3. Upload packages and checksums by release ID without writing the body, changing the draft flag, or creating a second release. Remove `softprops/action-gh-release` from these two CD paths unless a pinned version is fixture-proven to reuse the same draft safely.
4. Attach the attribution manifest and optional visual to the same release ID.
5. Let the **release finalizer job** be the only writer of the final body and the only job allowed to change `draft: true` to published. It waits for all required product artifacts and attribution checks, marks Cua Driver as a prerelease, and allows Lume to become the latest release.
6. Verify that the release body matches the tag-pinned changelog data and that the manifest tag and SHA match the release target.
7. Publish the GitHub release and produce the non-blocking social draft artifact.

All jobs that mutate a release use a per-tag concurrency group. During the
pilot, rendering failures leave the release in draft along with artifact,
checksum, manifest, attribution, and body-parity failures. We can split optional
visual generation into a non-blocking job after the first live fixtures prove
the shared renderer and publishing path.

## Validation rules

The pipeline should fail before publishing when:

- A product-changing pull request title does not follow the agreed Conventional Commit form.
- A release-producing title has an unsupported type or malformed breaking-change marker.
- The Release Please version differs from Cua Driver's `rust/VERSION`, `[workspace.package].version`, any workspace-member version in `Cargo.lock`, Lume's `VERSION`, or Lume's source constant.
- A human external pull request author is missing from the contribution record.
- A known human coauthor is missing.
- A landing commit incorporates an external source pull request without preserving its author, recording a coauthor, or carrying the contributor into the release manifest.
- A bot appears in a contributor thank-you.
- The same pull request appears more than once.
- A pull request falls outside the selected tag range.
- The changelog and manifest disagree.
- The target tag already exists at a different SHA, or any step attempts to delete, overwrite, or move it. A rerun for the same tag and SHA must be idempotent.
- Generated release notes exceed GitHub's body limit without a safe compact form.

Tests should cover identity resolution, coauthor parsing, bot filtering, path matching, multi-product pull requests, bump selection, Markdown rendering, JSON schema stability, and deterministic output.

### Required fixture matrix

Run these scenarios in a fork or throwaway branch with the action pinned to the intended commit. Every row is a cutover gate.

| Scenario | Fixture | Pass criteria |
| --- | --- | --- |
| Driver patch | Merge `fix(driver): ...` under the driver release scope | `0.8.3` becomes `0.8.4` in the manifest, `rust/VERSION`, Cargo workspace version, lockfile entries, and changelog. |
| Lume minor | Merge `feat(lume): ...` | `0.3.16` becomes `0.4.0`; `VERSION` and `Main.swift` remain equal in the release pull request. |
| Pre-1.0 breaking change | Merge `feat(driver)!: ...` | The resulting version matches the team's documented policy: `1.0.0` by default, or `0.9.0` only if `bump-minor-pre-major` is deliberately enabled. |
| No release | Merge `docs`, `test`, or excluded-path-only changes | No product release pull request is opened or updated. |
| Installer-only driver change | Change `libs/cua-driver/scripts/**` | The Cua Driver release history includes it; excluded Python/docs/test changes do not trigger the component alone. |
| Cross-product change | One squash commit touches both component scopes | The entry and bump type appear in both release pull requests; merging either does not disturb the other. |
| External author and coauthor | Use an external PR author, a human `Co-authored-by` trailer, and linked reporter | Verified handles and roles appear once; missing salvage credit or a thanked bot fails validation. |
| Tag and draft integration | Merge a release pull request | The tag exists immediately at the release SHA, the release remains draft, and the App-token-created tag triggers CD. |
| Draft asset upload | Complete CD against that tag | Assets land on the same release ID; no second release appears; the body and draft state remain unchanged. |
| Failed artifact | Fail one required build | The release remains draft; a rerun uploads assets idempotently and does not duplicate the release. |
| Publication | Complete every required job | The final body and manifest match the tag and SHA; Cua Driver is prerelease, Lume latest behavior is correct, and the finalizer publishes exactly once. |
| Legacy coexistence | Merge a PR carrying an old `release:cua-driver-rs` or `release:lume` label during pilot | No legacy `bump2version` release is triggered. |

## Rollout

### Phase 0: Release safety

- Add per-product and per-tag workflow concurrency to both CD workflows.
- Remove remote tag deletion from `release-bump-version.yml`'s `clean_tag` function and API release path. Do not remove the local rebase-retry bookkeeping while the legacy workflow still exists.
- Fail when a target tag exists at a different SHA; allow a same-tag/same-SHA rerun to resume safely.
- Keep Cua Driver and Lume tags independent.
- Add regression tests for tag behavior.

**Exit condition:** A release tag points to one commit and cannot move through the supported workflow.

### Phase 1: Attribution foundation

- Add `release-please-config.json` and seed `.release-please-manifest.json` with the current versions.
- Pin `googleapis/release-please-action` by commit SHA and validate the config with its embedded Release Please version.
- Add `libs/cua-driver/rust/VERSION`, the targeted Cargo workspace update, and the release-PR precise lockfile sync.
- Configure Lume's existing `VERSION` file and annotated `Main.swift` constant.
- Add Conventional Commit pull request title validation and enforce squash merges for product-changing pull requests.
- Add the authorship preservation policy to `CONTRIBUTING.md` and a repository-level `AGENTS.md` after team approval.
- Run the complete fixture matrix with separate component release pull requests.
- Prove that the Cua release GitHub App token triggers the existing tag workflows.
- Build the GitHub attribution checker and JSON manifest renderer.
- Replace the existing `git log` note generation in both CD workflows.

**Exit condition:** Release Please produces correct release pull requests for both components, and a historical tag-range test produces complete notes with verified GitHub handles.

### Phase 2: Ship through the new path

- Remove Cua Driver and Lume from the service inputs in `release-bump-version.yml`.
- Remove both path mappings and disable the old `release:lume` and `release:cua-driver-rs` triggers in `release-on-merge.yml`.
- Update `ci-release-reminder.yml` and `release-unreleased-digest.yml` so neither points these products at the legacy release path.
- Merge one Release Please release pull request for each component.
- Replace each `softprops/action-gh-release` creation step with release-ID-based uploads to the Release Please draft.
- Attach manifests and verify release-body parity.
- Ship one patch release for each product.
- Review attribution with the contributors named in those releases.

**Exit condition:** Cua Driver and Lume each publish a release through the new pipeline with no manual identity correction after publication.

### Phase 3: Visual and announcement drafts

- Create one visual template for Cua Driver and one for Lume.
- Render a release-card preview from the manifest.
- Add review and approval before attaching the card.
- Render an X post or thread draft with alt text.
- Reuse the GitHub release asset in the social draft.

**Exit condition:** A selected minor or major release publishes one reviewed card and produces an approved X draft from the same manifest.

### Phase 4: Broader adoption

- Review the first releases and contributor feedback.
- Decide whether to include more packages.
- Add cloud consumption only if another system needs the structured feed.
- Consider approved posting automation after several successful draft-only releases.

## Proposed defaults for team review

- Use `cua-driver-rs` as the internal release key and `Cua Driver` as the public name.
- Credit external authors, human coauthors, and directly linked issue reporters.
- Preserve external authorship through direct merges, `git cherry-pick -x`, or `Co-authored-by` trailers for materially reused work.
- Keep internal maintainer work in the change record; reserve the public thank-you sentence for external contributors.
- Use Release Please manifest mode, Conventional Commit squash titles, and GitHub identities.
- Generate a visual for major releases and selected minor releases.
- Keep social posting manual during the first rollout.
- Keep release identity logic in the core repository.
- Let the release finalizer job publish only after required CD and attribution jobs succeed.

## Decisions and remaining approvals

The implementation records internal maintainers in the manifest and reserves
the public thank-you sentence for external contributors. It credits reporters
of explicitly closed same-repository issues. Major releases and pull requests
with the `release-visual` label request a card. A pre-1.0 breaking change uses
Release Please's default major bump and therefore advances to `1.0.0`.

The team still needs to decide:

1. Who approves the final release card and social copy?
2. Where should editable visual templates live, and which team owns brand changes?
3. Which X account and posting tool would a later approved automation use?

## Success measures

For the first three releases of each product, record:

- Eligible external contributors found versus contributors credited
- Manual identity corrections needed after generation
- Pull requests omitted or duplicated
- Time spent preparing notes
- Time spent preparing the release visual and social draft
- Contributor corrections requested after publication
- Release-body or tag drift detected by validation

The target is complete external contributor credit, zero moved release tags, zero omitted pull requests, and no post-publication identity fixes.

## Definition of done for the initial project

The initial project is complete when:

- Cua Driver and Lume use Release Please manifest mode with separate component release pull requests.
- Both products generate release notes through Release Please and attribution manifests from GitHub pull request data.
- Both products publish immutable tags, tag-pinned changelogs, and tag/SHA-bound tamper-evident manifests.
- Every eligible external author and known coauthor receives credit.
- Salvaged external work preserves git authorship or an explicit coauthor record and source pull request link.
- Both CD workflows consume the generated release body.
- Renderer and validation tests pass in CI.
- Each product has shipped one release through the new path.
- One selected feature release has produced a reviewed GitHub release card and X draft.

## References

- [Hermes release script](https://github.com/NousResearch/hermes-agent/blob/main/scripts/release.py)
- [Hermes v2026.6.5 Surface Release with infographic](https://github.com/NousResearch/hermes-agent/releases/tag/v2026.6.5)
- [OpenClaw changelog update process](https://github.com/openclaw/openclaw/blob/main/.agents/skills/openclaw-changelog-update/SKILL.md)
- [OpenClaw changelog attribution checker](https://github.com/openclaw/openclaw/blob/main/scripts/check-changelog-attributions.mjs)
- [OpenClaw GitHub release-note renderer](https://github.com/openclaw/openclaw/blob/main/scripts/render-github-release-notes.mjs)
- [Release Please manifest mode](https://github.com/googleapis/release-please/blob/main/docs/manifest-releaser.md)
- [Release Please customization and generic version updates](https://github.com/googleapis/release-please/blob/main/docs/customizing.md)
- [Release Please GitHub Action](https://github.com/googleapis/release-please-action)
- [GitHub generated release notes](https://docs.github.com/en/repositories/releasing-projects-on-github/automatically-generated-release-notes)
- [Changesets monorepo configuration](https://github.com/changesets/changesets/blob/main/docs/config-file-options.md)
- [dist release and distribution workflow](https://github.com/axodotdev/cargo-dist)
- [Current Cua Driver release workflow](../.github/workflows/cd-rust-cua-driver.yml)
- [Current Lume release workflow](../.github/workflows/cd-swift-lume.yml)
- [Current version bump workflow](../.github/workflows/release-bump-version.yml)
