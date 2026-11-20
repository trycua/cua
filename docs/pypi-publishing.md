# Publishing to PyPI: build often, promote weekly

Cua Driver and the cua SDK ship many builds a week. Every build goes to its
GitHub release and to our installers. PyPI gets one promoted build a week per
component, or more when a maintainer promotes by hand.

| Component  | PyPI projects     | GitHub release tag | Build workflow                         |
| ---------- | ----------------- | ------------------ | -------------------------------------- |
| Cua Driver | `cua-driver`      | `cua-driver-rs-v*` | `cd-rust-cua-driver.yml`, then `cd-py-cua-driver.yml` |
| cua SDK    | `cua-cli`, `cua`  | `cua-sdk-v*`       | `cd-cua-sdk.yml`                       |

## The promotion rule

**Every Monday at 15:00 UTC, `pypi-promote.yml` uploads the newest promotable
build of each component to PyPI.** A build is promotable when:

- its GitHub release is published (not a draft) with a plain `X.Y.Z` version;
- it is at least 24 hours old, so a broken build gets replaced first;
- its build workflow has a successful run for the tag;
- every expected wheel is attached to the release and matches its checksums
  file (for the cua SDK, `cua` requires `cua-cli` at the same version, and
  `cua-cli` is uploaded first).

If PyPI already has that version, or a newer one, the run does nothing.

Cua Driver releases are published as GitHub prereleases. That is unrelated
to PyPI: the weekly promotion picks them up all the same.

## Promote now

Any of these uploads a build to PyPI right away:

- **Run the workflow by hand.** Actions, **Release: promote to PyPI**, Run
  workflow. Leave `version` empty for the newest promotable build (no 24 hour
  wait), or give an exact `X.Y.Z`. `dry_run` downloads and checks the wheels
  without uploading.
- **Mark a release as stable.** Editing a Cua Driver release on GitHub and
  clearing "Set as a pre-release" promotes that release.

```bash
gh workflow run pypi-promote.yml -R trycua/cua -f component=cua-driver
gh workflow run pypi-promote.yml -R trycua/cua -f component=cua-sdk -f version=0.25.2
```

## Change the cadence

- **Pause the weekly run:** set the repository variable
  `PYPI_PROMOTE_SCHEDULE` to `off`. Manual promotion still works.
- **Change the day or time:** edit the `cron` line in
  `.github/workflows/pypi-promote.yml`.

## PyPI never goes quiet unnoticed

The weekly run fails when PyPI's newest release of a component is more than
14 days behind the newest GitHub build, including while the schedule is
paused or every recent build is red. The failure names the component and the
PyPI version it is stuck on. Promote a build by hand, or fix the builds.

## Why

A Cua Driver release is five platform wheels of 27 to 41 MB, a cua SDK release
twelve wheels of 51 to 82 MB, and PyPI caps each project at 10 GB. At 15 to 18
builds a week, every one on PyPI, `cua-driver` hit the cap on 10 Nov 2026 and
`cua` was right behind it. Promoting weekly cuts PyPI uploads by over 90% while
GitHub releases and the installers keep every build. Old releases are removed
only under the PyPI retention rule (`docs/pypi-retention.md`).
