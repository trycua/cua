# Releasing Cua Spaces: Stable and Nightly

Cua Spaces has two update channels. There are no betas.

| Channel | Who gets it | Where it comes from |
| --- | --- | --- |
| Stable (default) | Everyone | Release Please opens the `cua-spaces` release PR; merging it tags `cua-spaces-vX.Y.Z` and `cd-cua-spaces.yml` builds, verifies (staging smoke and canary gate) and publishes the release and the stable feeds on `cua-spaces-latest`. |
| Nightly | Apps set to Nightly in Settings, Advanced | `nightly-cua-spaces.yml` builds main every night and replaces the assets of one rolling prerelease, `cua-spaces-nightly`. |

## Testing before a release

Don't tag a beta. To test what is on main:

- install or switch to the Nightly channel (Settings, Advanced, Nightly), or
  download a nightly directly from the
  [`cua-spaces-nightly` release](https://github.com/trycua/cua/releases/tag/cua-spaces-nightly)
  (the `cua-spaces-nightly-*` files keep their names every night); or
- run the **Nightly: Cua Spaces** workflow by hand (Actions, Run workflow) to
  build main now. `force` builds even when nothing changed; `ignore_ci`
  builds even when main's Spaces CI is red or still running.

## How the nightly works

- **When:** every day at 07:30 UTC, and on a manual run. It skips when the
  release inputs haven't changed since the last nightly, or when main's
  Spaces CI (`ci-cua-spaces*.yml`) is red, or still running after 90 minutes.
- **What:** the same signed builds as a release: macOS notarized and
  stapled, Windows Authenticode signed, Linux AppImage and deb.
- **Version:** `X.Y.Z-nightly.YYYYMMDD.N+sha7`, where X.Y.Z is the next
  minor after main's version and N is the workflow's run number, so every
  nightly is unique and newer than the one before, even two on one day. The
  Windows and Linux apps carry it without `+sha7`.
- **Feeds:** all on `cua-spaces-nightly`, never on `cua-spaces-latest`.
  - macOS: `cua-spaces-nightly-appcast.xml`, every item on Sparkle's
    `nightly` channel.
  - Windows: `nightly.yml`.
  - Linux: `nightly-linux.yml` and `nightly-linux-arm64.yml`.
  - Stable apps read `cua-spaces-appcast.xml` and `latest*.yml` on
    `cua-spaces-latest`, so they never see a nightly. Each run checks this
    after it publishes, and checks that every download link resolves.
- **Immutable tag:** after a fully green build and publish, the workflow also
  tags the built commit `cua-spaces-nightly-YYYYMMDD.N` (the same date and run
  number as the version). The tag is created once at the exact built commit
  and never moved; if it exists elsewhere the job fails. `nightly.json` and the
  release notes name the tag and commit. A red nightly leaves no tag. Use it to
  read or pin the code of a nightly.
- **Never Latest:** the release is a prerelease created with
  `--latest=false`, and its tag never matches `cua-spaces-v*`, so no release
  workflow runs and no draft is left behind.
- **Failures:** a platform that fails keeps last night's build, the other
  platforms still publish, and the run ends red.

Cua Driver and Lume use immutable per-night tags
([RFC 3096](../rfcs/3096-immutable-nightly-release-channels.md)). Spaces uses
one rolling release on purpose: the in-app updaters read a fixed feed URL,
and 29 beta tags in a month showed that per-build releases pile up.

## The beta guard

`cd-cua-spaces.yml` refuses any `-beta` version (tag push or manual run) in
its first job, before anything is built or a draft release is created. To
allow one in an emergency, set the repository variable `ALLOW_SPACES_BETA` to
`true` (Settings, Secrets and variables, Actions, Variables), then delete it
right after. Betas published before November 2026 remain as prereleases;
their items in the beta feeds are no longer updated.

## Rebuilding a tag whose run never published

If a tag exists but its tag-push run never published (for example the workflow
itself was broken), do not move or delete the tag. Dispatch the workflow from
`main`, which carries the fix, with the tag and its commit:

```sh
gh workflow run cd-cua-spaces.yml -R trycua/cua --ref main \
  -f rebuild_tag=cua-spaces-vX.Y.Z -f rebuild_sha=<full commit SHA of the tag>
```

The run builds the tagged commit with `main`'s workflow files and publishes
the existing draft release (or creates it for that tag). It refuses a release
that is already published, a tag that does not point at `rebuild_sha`, a
non-stable tag, and a dispatch from any other branch. Every gate runs exactly
as on a tag push: signing, notarization, the staging smoke for that commit,
`verify-release`, the cua.ai check and the staged rollout. `electron_dry_run`,
`sparkle_electron`, `skip_staging_gate` and `platforms` are refused. Its
Sigstore identity is the workflow at `refs/heads/main`, which says nothing
about the tag, so the installers accept it only with a signed claim: the run
signs `rebuild-claim.txt` (a `tag cua-spaces-vX.Y.Z` line and the sha256 of
every asset) and uploads it with its bundle next to the assets. The installers
check the claim and the artifact under that identity, the `workflow_dispatch`
trigger and the `main` ref, then require the claim to name the tag being
installed and list the artifact's sha256. A release rebuilt by a workflow
without the claim step (before this check) does not verify.
