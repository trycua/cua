# Release components with Release Please

This runbook covers the normal maintainer release path for Cua Driver and Lume.
Both components use Release Please, but their publication steps differ.

| Component  | Source path       | Release Please component | Tag prefix        | Publisher                                  |
| ---------- | ----------------- | ------------------------ | ----------------- | ------------------------------------------ |
| Cua Driver | `libs/cua-driver` | `cua-driver-rs`          | `cua-driver-rs-v` | `.github/workflows/cd-rust-cua-driver.yml` |
| Lume       | `libs/lume`       | `lume`                   | `lume-v`          | `.github/workflows/cd-swift-lume.yml`      |

The workflows in this table are the source of truth. Historical plans explain
why the system exists, but they do not replace this runbook or the workflow
files.

## Before you begin

Confirm that:

- the product pull request is merged;
- its final title has the intended Conventional Commit type and component;
- its required checks and component-specific acceptance evidence passed;
- the release owner has authorized publication; and
- no existing component tag names the proposed version.

Release Please maps `fix` to a patch and `feat` to a minor. A breaking marker
uses the configured pre-1.0 policy. Non-releasing types such as `docs`, `test`,
`chore`, and `ci` do not create a component release entry. See the release-title policy in
[`AGENTS.md`](../AGENTS.md#pull-request-titles-and-component-releases).

Never move, replace, or delete a release tag. If tagged product code is wrong,
fix it in a new pull request and release a new version.

## 1. Let Release Please prepare the release

Every push to `main` runs `Release: Prepare component releases`. Release Please
opens or updates one pull request per component. It also rebases trusted release
branches and synchronizes generated version files.

Check the preparation run and locate the component pull request:

```bash
gh run list --repo trycua/cua \
  --workflow release-please.yml --branch main --limit 10

# Cua Driver
gh pr list --repo trycua/cua --state open \
  --head release-please--branches--main--components--cua-driver-rs

# Lume
gh pr list --repo trycua/cua --state open \
  --head release-please--branches--main--components--lume
```

Use the manual `workflow_dispatch` inputs only to repair or deliberately target
a release. `automatic` is the normal bump choice. A forced bump is a release
decision, not a way to compensate for an incorrect product pull request title.

```bash
gh workflow run release-please.yml --repo trycua/cua --ref main \
  -f component=cua-driver-rs -f bump=automatic
```

## 2. Review the release pull request

Set `PR` to the Release Please pull request number, then inspect its live state:

```bash
gh pr view "$PR" --repo trycua/cua \
  --json title,isDraft,mergeable,mergeStateStatus,reviewDecision,headRefOid,baseRefOid
gh pr diff "$PR" --repo trycua/cua --name-only
gh pr diff "$PR" --repo trycua/cua
gh pr checks "$PR" --repo trycua/cua
```

Require all of the following:

- the title names the expected component and version;
- the changelog contains the intended product commits and SemVer effect;
- the manifest changes only that component's version;
- generated version files agree;
- the branch is current and mergeable;
- required checks pass; and
- there are no requested changes or unresolved review threads.

For Cua Driver, the version must agree in the Rust workspace, `rust/VERSION`,
Cargo lockfile, Python package, TypeScript package and lockfile, bundled driver
skill, generated reference docs, and `.release-please-manifest.json`.

For Lume, the version must agree in `VERSION`, `src/Main.swift`, the installer,
generated reference docs, changelog, and release manifest.

Do not hand-edit the release branch to correct a product entry. Fix the merged
pull request body with the supported commit override when appropriate, then
rerun Release Please. Replace the example entry with one or more Conventional
Commit entries that Release Please should use for that squash commit:

```text
BEGIN_COMMIT_OVERRIDE
fix(cua-driver): preserve keyboard input while the driver reconnects
END_COMMIT_OVERRIDE
```

## 3. Merge the component release pull request

Merge only the component that is ready to ship. Perform the same fresh checks
immediately before merging.

```bash
gh pr merge "$PR" --repo trycua/cua --squash
```

Repository policy may allow an authorized maintainer to bypass a missing
approval. It never allows bypassing failed checks, unresolved feedback, an
unsafe diff, or a merge conflict.

After the merge, Release Please creates the immutable component tag and a draft
GitHub release. Fetch the tag and prove that it points at the release merge:

```bash
VERSION=0.19.3
TAG="cua-driver-rs-v${VERSION}"

git fetch --tags origin
git rev-list -n 1 "$TAG"
gh release view "$TAG" --repo trycua/cua \
  --json tagName,isDraft,isPrerelease,publishedAt,targetCommitish,url,assets
```

## 4. Publish Cua Driver

Cua Driver uses a two-stage path. The tag push first builds and verifies an
immutable candidate. It does not publish the draft.

Find and watch the tag-triggered candidate run:

```bash
gh run list --repo trycua/cua \
  --workflow cd-rust-cua-driver.yml --branch "$TAG" --limit 5
gh run watch "$RUN_ID" --repo trycua/cua --exit-status
```

Require the attribution preflight, platform builds, Windows x64 and ARM64 native
package tests, archive contract, and packaged MCP discovery to pass.

Before publication, account for the canonical desktop E2E evidence described in
[`libs/cua-driver/docs/test-harnesses-guide.md`](../libs/cua-driver/docs/test-harnesses-guide.md).
Certify the exact executable candidate. If the Release Please merge changed only
generated version metadata, carry forward unaffected evidence from the certified
product commit and record the final diff. Rerun every lane affected by executable,
harness, generated-contract, or environment changes.

Once the candidate and required desktop evidence pass, dispatch the explicit
publish run from current `main`. The workflow checks out source from the immutable
tag while using current release-control helpers.

```bash
gh workflow run cd-rust-cua-driver.yml --repo trycua/cua --ref main \
  -f version="$VERSION" -f notarize=true -f publish=true

gh run list --repo trycua/cua \
  --workflow cd-rust-cua-driver.yml --event workflow_dispatch --limit 5
gh run watch "$PUBLISH_RUN_ID" --repo trycua/cua --exit-status
```

The successful publish run uploads the release assets, publishes the verified
draft, and advances the baked installer version on `main`. It also triggers
`CD: Cua Driver SDK packages`, which publishes the Python wheels, TypeScript SDK,
and platform-native npm packages from the same tag.

Watch the SDK publisher before calling the component shipped:

```bash
gh run list --repo trycua/cua \
  --workflow cd-py-cua-driver.yml --event workflow_run --limit 10 \
  --json databaseId,createdAt,event,headSha,status,conclusion
gh run view "$SDK_RUN_ID" --repo trycua/cua --json jobs,status,conclusion
gh run watch "$SDK_RUN_ID" --repo trycua/cua --exit-status
```

The candidate completion also starts this workflow, but its `get-version` job
sets `should_publish=false` and skips publication. Select the later SDK run
chained from the explicit publish dispatch. Require its wheel and npm build and
publish jobs, not merely the candidate-triggered no-op run, to succeed.

## 5. Publish Lume

Lume uses a single tag-triggered publication run. Its CD workflow builds,
notarizes, uploads assets, and publishes the verified Release Please draft.

```bash
# Example only; replace this with the version in the Lume release pull request.
VERSION=0.5.2
TAG="lume-v${VERSION}"

gh run list --repo trycua/cua \
  --workflow cd-swift-lume.yml --branch "$TAG" --limit 5
gh run watch "$RUN_ID" --repo trycua/cua --exit-status
```

Require the checked-in versions, notarized assets, attribution manifest, release
body, and published release to agree with the tag.

## 6. Verify distribution

Check the component release directly. Do not use the repository-wide Latest
release badge or `/releases/latest` for Cua Driver.

For Cua Driver:

```bash
gh release view "$TAG" --repo trycua/cua \
  --json tagName,isDraft,isPrerelease,publishedAt,targetCommitish,url,assets

npm view @trycua/cua-driver version
npm view @trycua/cua-driver-win32-arm64-msvc version
python3 -m pip index versions cua-driver

git fetch origin main
git show origin/main:libs/cua-driver/scripts/_install-rust.sh | \
  rg 'CUA_DRIVER_RS_BAKED_VERSION='
git show origin/main:libs/cua-driver/scripts/install.ps1 | \
  rg 'CuaDriverRsBakedVersion ='
```

Require the expected archives, checksums, installers, skill pack, SDK packages,
and native packages. Confirm that both canonical installers resolve the released
component version. Cua Driver releases intentionally use GitHub's prerelease flag;
the component tag and canonical installer resolution determine whether the
component has shipped.

For Lume:

```bash
gh release view "$TAG" --repo trycua/cua \
  --json tagName,isDraft,isPrerelease,publishedAt,targetCommitish,url,assets

git fetch origin main
git show origin/main:libs/lume/scripts/install.sh | rg 'LUME_BAKED_VERSION='
git show origin/main:libs/lume/src/Update/VersionCheck.swift | \
  rg 'releaseTagPrefix|defaultInstallScriptURL'
```

Require the exact tagged GitHub release, notarized archives, baked installer
version, and updater tag prefix and canonical installer path to agree. Unlike
Cua Driver, Lume's installer may use the repository-wide latest release as a
fallback after its explicit and baked-version paths; verify the exact tagged
release and baked version rather than relying on that fallback alone.

## Failure recovery

- If a candidate build fails, leave the release draft and diagnose the failed
  job. Do not publish partial artifacts.
- If current release-control code can safely recover an immutable tag, fix that
  code on `main` and rerun the supported workflow dispatch for the same tag.
- If tagged product code or package contents are wrong, land a product fix and
  release a new version. Never move the existing tag.
- If a registry publish partially succeeds, inventory each exact package and
  version before retrying. Registries treat published versions as immutable.
- Record the tag, tag SHA, candidate run, desktop evidence, publish run, SDK run,
  and final distribution checks in the release handoff.
