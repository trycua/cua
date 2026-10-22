---
title: Desktop release model, build once and promote later
authors:
  - f-trycua
created: 2026-10-21
status: draft
linear: CUA-1991
implementation:
  - https://github.com/trycua/cua/pull/5230
reviewers:
  - ddupont808
  - f-trycua
---

# Desktop release model: build once, promote later

## Summary

Spaces (and later Cua Driver) will ship the way VS Code, Cursor, Tailscale
and Chrome do.

1. **Build once, promote later.** Every push to main that can ship builds,
   signs and notarizes the installers for each platform. A release does not
   rebuild. It verifies and publishes the artifact set already built for that
   commit.
2. **Platform patch releases.** One shared version line. A macOS-only hotfix
   publishes only the macOS build. Other platforms skip that number and keep
   their previous build in the feeds.
3. **Staged rollout.** Updater feeds serve a new stable build to 10%, then 50%,
   then 100% of installs, with a pause switch.

Target: a macOS-only fix goes from code-ready to live in about 2 hours,
down from about 5. Scope is Spaces first. Driver follows once Spaces has run
this way for a few releases.

## Why now

Today a tag push builds everything from scratch in `cd-cua-spaces.yml`. Tag to
published has a median of 86 minutes, 5 of 23 tag runs failed and 3 were
cancelled in 14 days, and a failure after the tag costs a whole new run (see
`release-speed-audit-2026-10-19`). The build is also not tested before the tag
exists: the staging gate (CUA-1907) fails on the tag run and someone re-runs it
after posting the smoke status.

## Pattern

```
push to main ──> build workflow (signed, notarized, attested)
                    artifacts + build-manifest.json (per commit)
                           │
staging smoke runs on those exact artifacts, posts status with manifest digest
                           │
release PR merges ──> that merge commit's build ──> tag on the same commit
                           │
tag run (cd-cua-spaces.yml) = promote:
   verify attestations + digests, verify-release.sh, staging gate,
   cosign the bytes, publish release, write feeds at 10%
                           │
rollout workflow: 10% -> 50% -> 100%, or pause
```

Release Please bumps `apps/cua-spaces-macos/VERSION` in the release PR, so the
merge commit already contains the final version. Its main build is therefore
the release build. The tag is created on that commit after the smoke is green,
instead of before.

### Where things live

| Thing | Location |
|---|---|
| Built, signed installers per commit | Actions artifacts of the main build run, `spaces-build-<sha>` |
| `build-manifest.json` | Same artifact set. Lists every file, sha256, platform, version, build number, source SHA and tree hash |
| Attestations | GitHub artifact attestations (SLSA provenance) for every file, signer workflow `cd-cua-spaces-build.yml@refs/heads/main` |
| Released installers | The GitHub Release, as today (permanent) |
| Sigstore bundles for installers and feeds | Created by the promote job in `cd-cua-spaces.yml` at the tag, so the identity `install.sh` and `install.ps1` verify does not change |
| Staging smoke evidence | Commit status `staging-smoke/cua-spaces`, now carrying the manifest digest |
| Feeds | Rolling `cua-spaces-latest` release (appcast, `latest*.yml`) and `cua-install-latest` (`release-artifacts.json`), as today |

### Signing and secrets

The build workflow runs only on push to main and uses the
`cua-driver-release-signing` environment, restricted to the `main` branch. Pull
requests never see signing secrets. Changes to the build workflow need
CODEOWNERS review. Signing identities, notarization, Sparkle EdDSA key and
Azure Artifact Signing are the same ones the tag run uses now.

## How the certification stays exact (does not weaken)

Every existing gate stays, and two get stricter.

- **Staging-first (CUA-1907).** The gate still requires a green
  `staging-smoke/cua-spaces` status for the release SHA. New: the status
  description carries the sha256 of `build-manifest.json`, and the gate fails
  unless it equals the digest of the artifact set being promoted. Today the
  digest is only displayed. A smoke of other bytes, even on the same commit,
  no longer passes. Break-glass (`skip_staging_gate` with a reason) is
  unchanged and still logged loudly.
- **Signature verification.** The promote job downloads the artifact set and
  runs `verify-release.sh` on those bytes before anything is published, as
  `verify-release` does now (Gatekeeper, stapled tickets, strict codesign,
  Authenticode by the expected signer, appcast, feeds, sha512). On top of that
  it runs `gh attestation verify` pinned to the build workflow and `refs/heads/main`,
  and checks every digest against the manifest.
- **Version check (new).** Promotion fails if the version baked into any
  artifact differs from the tag version, or if the manifest's source SHA is not
  the tag's commit.
- **No gap.** If no complete, attested artifact set exists for the tag's
  commit, the tag run falls back to today's full build. Nothing is ever
  published without passing these checks.

## Platform patch releases

Version stays one line (`0.11.1`). The release plan (the existing
`electron-plan` job, extended) decides the platforms from changed paths:

- Shared paths (`libs/cua/**`, `apps/cua-spaces-desktop/src/**` renderer code,
  the web UI, `cua-spacesd` pin) mean all platforms.
- `apps/cua-spaces-macos/**` alone means macOS only. Windows or Linux packaging
  paths alone mean those platforms only.
- A manual `platforms` input can override this, with a reason.

For a platform that is skipped:

- Its installers are not rebuilt or copied. The new release notes say which
  platforms it contains.
- Its updater feed is not touched: `latest.yml` and `latest-linux*.yml` keep
  pointing at the previous build, so those users are not offered anything.
  Sparkle gets a new item only when macOS is in the release.
- `release-artifacts.json` and the `cua-install-latest` manifest already merge
  per platform. Each entry keeps its own version, so `install.sh` and
  `install.ps1` install the newest build that exists for their platform. Phase 3
  adds a test that they accept mixed versions.

## Staged rollout

Stable builds only. Beta stays at 100%.

- **electron-updater (Windows, Linux, later macOS Electron).** Use the native
  `stagingPercentage` field in `latest*.yml`: 10, then 50, then 100. Pause
  means setting the feed back to the previous version's file. Users who already
  updated are not downgraded.
- **Sparkle (macOS SwiftUI).** Use Sparkle's native phased rollout
  (`sparkle:phasedRolloutInterval` on the appcast item) so clients take the
  update in groups over time. Pause means removing the item from the appcast.
  Sparkle cannot downgrade, so rollback is a fixed higher-numbered release.
  Open point: Sparkle's groups are time-based, not a fixed 10/50/100, so
  phase 4 starts by confirming how closely it can match, and accepts the
  difference if the native behavior is close enough. We do not build a
  custom update server.
- **Control.** One `workflow_dispatch` workflow, `release-rollout`, with inputs
  component, platform and `10 | 50 | 100 | pause`. It edits the feed assets on
  `cua-spaces-latest` and logs who did it. Advancing is manual at first. The
  one-click install links and `cua-install-latest` move to the new build at
  100%.

"Live" in the 2 hour target means published at 10%.

## Alternatives considered

| Option | Why not (now) |
|---|---|
| Keep rebuilding at tag time | Slowest, untested bytes, the status quo |
| Cut a release branch per version (Chrome) | Fits a many-channel product. One version line with Release Please is enough |
| Artifacts as GitHub draft releases | Visible to every writer, easy to publish by mistake, no retention control |
| Object storage or OCI registry for artifacts | Extra service and credentials. Actions artifacts plus attestations meet the need for 7 to 30 day lifetimes |
| Own update server with percentage logic (Omaha, Squirrel server) | Standard feeds already support staged rollout. Avoids a new service |
| Build the release PR branch instead of main | Would expose signing secrets to PR branches |

## Retention and cost

- Main builds are kept 7 days. Builds of a version-bump commit are kept 30
  days. The release itself is permanent on the GitHub Release.
- Not every main push builds. The build runs when release inputs change
  (Spaces app, `libs/cua`, desktop app, signing scripts, `VERSION` files), is
  keyed by the tree hash of those inputs so an unchanged tree reuses the set,
  and runs one at a time, never cancelling a running build. Phase 1 measures
  the real macOS and Windows minutes and the artifact size per build; the
  go or no-go on cost is decided on that number, not an estimate.
- The cache cap blocker from #5230 (10 GB limit, entries evicted in minutes)
  must be fixed first, or main builds stay cold.
- Notarization is free but adds Apple-side latency. The existing retry helpers
  are reused.

## Risks

- Signing secrets run on every qualifying main push. Mitigated by the
  environment branch restriction and CODEOWNERS on workflows.
- Notarization or runner flakiness on main builds. Mitigated by the fallback
  to the full tag build, and by treating a red main build as a release blocker
  only for the version-bump commit.
- Build number (`CFBundleVersion X.Y.Z.<run number>`) now comes from the main
  run. The manifest records it and verify checks it.
- Sparkle phased rollout may not match 10/50/100 (see above).
- Mixed-version platforms confuse support. Release notes and the About page
  show the build's own version.
- Cost is unknown until phase 1 measures it.

## Phased rollout plan

0. **Warm the natives (open).** #5230 builds natives on main into the cache,
   no signing, no artifacts. Needs the cache limit raised and #5206. It is
   independent of the rest and shortens even today's tag run.
1. **Shadow build.** `cd-cua-spaces-build.yml` builds, signs, notarizes and
   attests on main and produces `build-manifest.json`. Nothing consumes it.
   Measure time, minutes, size and failure rate. Run `verify-release.sh` on it.
2. **Promote path.** `cd-cua-spaces.yml` promotes when a matching set exists,
   behind a repository variable, with the fallback. Staging gate checks the
   manifest digest. Try it on a beta tag, then a stable one.
3. **Per-platform patches.** Platform planner, feed carry-forward and the
   install-script mixed-version test. First real use: the next macOS-only fix.
4. **Staged rollout.** `release-rollout` workflow, `stagingPercentage` and the
   Sparkle phased item, with a rehearsal on the beta channel.
5. **Make it the default.** Remove the variable, keep the fallback for one
   more release, then decide whether Cua Driver follows.

Each phase ships on its own and can be switched off without losing the old
flow.

## Decisions wanted from review

- Spaces first, Driver later.
- Build on release-input changes only, not every push.
- Sparkle native phased rollout instead of a custom feed, if phase 4 shows it
  is close enough.
