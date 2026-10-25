# Release-channel component registry

`components.json` is the small, shared contract for immutable release channels.
It identifies a component's Release Please entry, disjoint stable and nightly
tag namespaces, build-time version sites, builder workflow, and change paths.
It intentionally does not describe signing, packaging, or registry publishing;
those remain owned by each component workflow.

## Release Please components and their release notes

Every package below gets a Release Please release PR, a generated
`CHANGELOG.md`, an immutable `<component>-v<version>` tag, and a GitHub
release whose body credits contributors. Release Please only prepares the PR,
changelog, tag and draft release; each package's own CD workflow (started by
the tag push) still builds and publishes.

The release body is the same one Cua Driver and Lume use:
`release_attribution.py collect` builds a pull-request-first manifest of the
tag range (authors, coauthors, and reporters of closed issues, resolved to
GitHub handles), and `render` writes Summary, Features/Fixes with per-change
thanks, a Contributors section that @-mentions every external contributor, and
the full-changelog link. GitHub draws the release's contributor avatars from
those @-mentions. Cua Driver and Lume render it inside their CD release job.
For every other component, `release-notes.yml` renders it when the stable tag
is pushed and replaces only the release body (draft or published), using
`release_notes.py`, which reads the package's path, exclusions and changelog
from `release-please-config.json`. On a release PR, `CI: Release metadata`
runs the same render first, so a changelog or attribution problem fails there
rather than at release time. If the job fails after tagging, the release keeps
Release Please's changelog; fix the cause and re-run **Release: Changelog and
contributor notes** with the tag.

| Package | Release Please path | Tag | Builds and publishes |
|---|---|---|---|
| Cua Driver (Rust, Python, npm) | `libs/cua-driver` | `cua-driver-rs-v*` | `cd-rust-cua-driver.yml` (renders its own notes) |
| Cua Perception | `libs/cua-driver/rust/crates/cua-perception` | `cua-perception-v*` | `cd-cua-perception.yml` |
| Lume | `libs/lume` | `lume-v*` | `cd-swift-lume.yml` (renders its own notes) |
| Cua Spaces (macOS, Electron, Tauri) | `apps/cua-spaces-macos` | `cua-spaces-v*` | `cd-cua-spaces.yml` |
| cua SDK and `cua` CLI | `libs/cua` | `cua-sdk-v*` | `cd-cua-sdk.yml` |
| cua-spacesd | `libs/cua-spacesd` | `cua-spacesd-v*` | `cd-cua-spacesd.yml` |
| cua-sandbox (PyPI) | `libs/python/cua-sandbox` | `sandbox-v*` | publish the draft; `cd-py-sandbox.yml` |
| cua-agent (PyPI) | `libs/python/agent` | `agent-v*` | `cd-py-agent.yml` |
| cua-core (PyPI) | `libs/python/core` | `core-v*` | `cd-py-core.yml` |
| cua-bench (PyPI) | `libs/cua-bench` | `bench-v*` | `cd-py-bench.yml` |
| cua-bench-ui (PyPI) | `libs/python/bench-ui` | `bench-ui-v*` | `cd-py-bench-ui.yml` |
| cua-som (PyPI) | `libs/python/som` | `som-v*` | `cd-py-som.yml` |
| cua-sandbox-apps (PyPI) | `libs/python/cua-sandbox-apps` | `sandbox-apps-v*` | `cd-py-sandbox-apps.yml` |
| @trycua/core (npm) | `libs/typescript/core` | `npm-core-v*` | `cd-ts-core.yml` |
| @trycua/playground (npm) | `libs/typescript/playground` | `npm-playground-v*` | `cd-ts-playground.yml` |
| cuabot (npm) | `libs/cuabot` | `cuabot-v*` | `cd-ts-cuabot.yml` |

The PyPI and npm packages above moved off the legacy bump2version workflow with
their tag formats and versions unchanged. Their CD workflows publish through
`release-github-reusable.yml`, which now uploads to and publishes the Release
Please draft for the tag instead of creating a second release (and still
creates one for a tag without a draft). Release Please bumps
`pyproject.toml`/`package.json` and the `uv.lock` entries that record the
package; `validate_release_versions.py --product packages` keeps them equal to
the manifest.

The first Release Please release after each package's last legacy tag (listed
in `release_notes.py` as `LEGACY_BASELINE_TAGS`) can have a short changelog:
Release Please searches only the newest 500 merges and drops the file list of
the 5,276-file launch merge (#4397). Its GitHub release notes still list every
change, because they come from git; the changelog-citation check resumes with
the next release. Check that release PR's version against the notes: for
cua-sandbox-apps, #4397 is a `feat`, so prepare it with
**Release: Prepare component releases** (component `sandbox-apps`, bump
`minor`) for 0.2.0 rather than the 0.1.3 Release Please proposes.

To release one of them: merge pull requests with conventional titles
(`fix(core): ...`, `feat(agent): ...`); Release Please opens or updates
`chore(main): release <component> <version>`. Review the version and changelog
and merge it. The tag push starts the CD workflow and `release-notes.yml`. To
force a bump, run **Release: Prepare component releases** with the component
and bump type. Cua Sandbox's draft is published by hand, which starts its PyPI
workflow; the others publish themselves.

Not on Release Please: the Docker images (`docker-*-v*`, still on
**Legacy packages: Bump Version**, with no GitHub release pages), `cua-fleet`
(Fleets is shut down), the OS images (`linux-image-v*`,
`omarchy-image-v*` and the image pipelines' own date versions), and the
`cua-install-latest`/`cua-spaces-latest` rolling feeds.

### Cua Spaces betas

Betas are cut by pushing a `cua-spaces-vX.Y.Z-beta.N` tag by hand on the
commit to test. `cd-cua-spaces.yml` creates a draft prerelease for it, builds,
signs and publishes it as a `beta` Sparkle item with its own notes. Release
Please ignores prerelease tags: it looks for the previous stable
`cua-spaces-v*` release, so betas never move the manifest version, never
appear in the release PR, and `release-notes.yml` skips them. When the soak
ends, merging the open `chore(main): release cua-spaces X.Y.Z` PR tags the
stable release from main, and its notes cover everything since the previous
stable release. Release Please 17.3.0 has no `additional-paths` option, so the
`cua-spaces` changelog counts commits under `apps/cua-spaces-macos` only.

## cua-perception releases

`cua-perception` has an independent version and changelog beside its Rust crate
in `libs/cua-driver/rust/crates/cua-perception`. Keeping the Release Please root
at that crate scopes its commits without unsupported path traversal. The
`.github/releases/cua-perception` directory remains the authority for schemas,
trust, and candidate controls. Release Please opens version PRs for the
component with `skip-github-release`, so it creates no GitHub release itself.
On each merged version, `release-please.yml` creates an idempotent lightweight
`cua-perception-v<semver>` tag on the exact main commit, using the release app
token so the tag push starts other workflows.

That tag push runs `cd-cua-perception.yml`. It builds the release input on each
target, passes it to the reviewed candidate workflow
(`cd-cua-perception-candidate.yml`) for verification, packaging, and signing,
installs every signed catalog with the published Cua Driver, and then publishes
the verified catalogs, archives, SBOMs, provenance, and install evidence as the
`cua-perception-v<semver>` GitHub release. The release is never marked Latest.
Nobody dispatches anything to publish. The signing job still runs in the
`cua-perception-candidate-signing` environment, so any protection rules on that
environment apply to each release.

The candidate workflow accepts prebuilt payloads. It does not download model
weights or compile product code. Every supplied worker, runtime, model, notice,
and source file must be declared with its exact size, SHA-256, license source,
target, and protocol version as applicable. Models and bundled source also need
complete ledgers. The workflow emits a deterministic archive, SPDX SBOM, signed
catalog, redacted provenance, and checksums as retained CI artifacts; only the
release workflow publishes them. `cd-cua-perception-candidate-manual.yml`
remains for staging candidates from a reviewed pull request head and never
publishes.

Candidate catalogs use the same Ed25519 envelope consumed by Cua Driver's
extension manager: `{ "payload": <compact catalog payload>,
"signature_algorithm": "ed25519", "signature": <base64> }`. The active public
key and rotation window are recorded in `cua-perception/trust-root.json`; the
matching private key is never generated,
stored, or uploaded by this repository. Candidate jobs receive it from an
external reviewed secret as base64-encoded PKCS#8 PEM and refuse keys whose
derived public key differs from the active trust root.

Candidate evidence is artifact-scoped. A passing metadata check is not a claim
that a future hosted or Fleet distribution satisfies a source-offer obligation.
Those distribution paths need their own review of the exact archive, durable
corresponding-source location, notices, and delivery behavior before any
compliance claim is made.

Cua Driver continues to exclude Perception paths from release attribution and
change detection. Shared Cargo manifest and lockfile changes are treated as
companions only when a commit otherwise contains Perception-owned changes. The
candidate tests also reject Driver archives containing Perception binaries,
model directories, or common model-weight formats.

To add a component, add one descriptor, keep its stable prefix identical to its
Release Please component tag, choose a unique `nightly-` prefix, declare only
version sites that affect built nightly artifacts, and add focused tests to
`test_release_channels.py`. Run:

```bash
python3 .github/scripts/release_channels.py validate
python3 -m pytest .github/scripts/tests/test_release_channels.py
```

Nightly versions are staged only in an ephemeral CI checkout. The script must
never rewrite stable baked installer defaults or published-version pointers.
Nightly release notes reuse the repository's PR-first attribution collector.
The first nightly is bounded by the component's current stable tag; later
nightlies are bounded by the previous published nightly. A component therefore
needs a reachable stable tag before its nightly channel is enabled.

Before starting platform builds, the nightly planner scans that exact range for
unresolved human coauthors. Unresolved attribution never holds a nightly: the
build continues, the plan carries `attributionWarning=true`, and the planner
writes a step summary and creates or refreshes one component-specific
maintainer issue. Nightly notes omit the unresolved coauthor. Stable releases
stay strict: `release_attribution.py` fails the manifest until the identity
resolves through a linked GitHub email or a verified `identityOverrides` entry;
never use wildcard or human-email ignores to clear it.

## Persistent consumer selection

Components that let users follow a channel should reuse the same consumer
contract while keeping product-specific installers and updaters:

- `stable` is the default when no preference exists;
- `channel set stable|nightly` persists intent without replacing the binary;
- installer `--channel stable|nightly` persists intent and installs that channel;
- exact immutable pins outrank saved state, are one-shot, and cannot be combined
  with an explicit channel;
- stable and nightly discovery use disjoint prefixes and strict version grammars;
- update caches include the selected channel, and structured status reports both
  the selected and current channel; and
- a channel mismatch is offered as an explicit transition even when ordinary
  SemVer ordering would point the other way.

Store the preference as a validated one-line `release-channel` file in the
component's existing product home. A missing file means stable; invalid or
unreadable state fails closed with a repair command. Product-home overrides must
relocate the file so installer and updater tests remain isolated. Cua Driver and
Lume are the reference implementations; [RFC 3101](../../docs/rfcs/3101-persistent-release-channel-selection.md)
records the full rationale and acceptance matrix.
