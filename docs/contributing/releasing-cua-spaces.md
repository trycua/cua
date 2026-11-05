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
- **Version:** `X.Y.Z-nightly.YYYYMMDD+sha7`, where X.Y.Z is the next minor
  after main's version. The Windows and Linux apps carry it without `+sha7`.
- **Feeds:** all on `cua-spaces-nightly`, never on `cua-spaces-latest`.
  - macOS: `cua-spaces-nightly-appcast.xml`, every item on Sparkle's
    `nightly` channel.
  - Windows: `nightly.yml`.
  - Linux: `nightly-linux.yml` and `nightly-linux-arm64.yml`.
  - Stable apps read `cua-spaces-appcast.xml` and `latest*.yml` on
    `cua-spaces-latest`, so they never see a nightly. Each run checks this
    after it publishes, and checks that every download link resolves.
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
