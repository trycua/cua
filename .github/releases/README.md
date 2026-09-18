# Release-channel component registry

`components.json` is the small, shared contract for immutable release channels.
It identifies a component's Release Please entry, disjoint stable and nightly
tag namespaces, build-time version sites, builder workflow, and change paths.
It intentionally does not describe signing, packaging, or registry publishing;
those remain owned by each component workflow.

## cua-perception candidates

`cua-perception` has an independent version and changelog beside its Rust crate
in `libs/cua-driver/rust/crates/cua-perception`. Keeping the Release Please root
at that crate scopes its commits without unsupported path traversal. The
`.github/releases/cua-perception` directory remains the authority for schemas,
trust, and candidate controls. Release Please may open version PRs for the
component, while `skip-github-release` keeps the stream candidate-only and does
not create GitHub releases. The workflow creates an idempotent lightweight
`cua-perception-v<semver>` git tag on the exact main commit carrying each version
so later candidate changelogs have an immutable range anchor.

The candidate workflow accepts prebuilt payloads. It does not download model
weights or compile product code. Every supplied worker, runtime, model, notice,
and source file must be declared with its exact size, SHA-256, license source,
target, and protocol version as applicable. Models and bundled source also need
complete ledgers. The workflow emits a deterministic archive, SPDX SBOM, signed
catalog, redacted provenance, and checksums as retained CI artifacts only.

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
unresolved human coauthors. It returns `reason=held-attribution` and creates or
refreshes one component-specific maintainer issue instead of spending build and
signing capacity on a candidate that cannot be published. Resolve the identity
through a linked GitHub email or a verified `identityOverrides` entry; never use
wildcard or human-email ignores to clear a hold.

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
Lume are the reference implementations; [RFC 3101](../../rfcs/3101-persistent-release-channel-selection.md)
records the full rationale and acceptance matrix.
