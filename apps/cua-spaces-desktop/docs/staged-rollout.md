# Staged rollout of Cua Spaces stable builds

CUA-2069, phase 4 of CUA-1991 (design: trycua/cua#5704, "Staged rollout").
A stable build is offered to a share of installs first. If it is bad, pause it
before everyone has it.

Run **Actions > Release: staged rollout (Cua Spaces)**. It edits only the feed
files on the rolling `cua-spaces-latest` release. Inputs:

| Input | Values |
|---|---|
| `component` | `spaces` |
| `platform` | `all`, `macos`, `windows`, `linux` |
| `stage` | `10`, `50`, `100`, `pause` |
| `reason` | free text, logged with the actor in the job summary |
| `channel` | `stable` (default), `beta` (rehearsal only) |
| `dry_run` | on by default: prints the feed diff, uploads nothing |

Typical flow after a stable release: `10`, watch crash and update telemetry,
`50`, `100`. Bad build: `pause`. The release workflow still publishes feeds as
before (everyone); writing them at 10% on publish comes with phase 2.

## Automatic advance (health gates, auto-pause)

`release-rollout-auto.yml` ("Release: auto rollout (Cua Spaces)") runs hourly and
does what the typical flow above asks of a person. It reads the stage from the
live feeds (`stagingPercentage` plus the newest appcast item's interval and
`pubDate`; stage 10 started at the `pubDate`, stage 50 at `pubDate` + 3 weeks)
and dispatches `release-rollout.yml` itself, so the actor and reason land in that
job summary as usual. Manual dispatch is unchanged.

| Decision | When |
|---|---|
| advance (10 to 50, 50 to 100) | stage at least 24h old, every gate has enough data and passes |
| wait | younger than 24h, or a gate has too little data (no data is not healthy) |
| pause | a gate is past its threshold, at any stage age: dispatches `pause` and posts an Alertmanager alert (`SpacesRolloutAutoPaused`, source `github-actions`, to #alerts-staging; it does not page) |
| hold | the variable `SPACES_ROLLOUT_HOLD` is set (the text is the reason) or an open issue has the label `spaces-rollout-hold`: never advances; a failing gate still pauses |
| none | 100%, paused (resuming is a person's call), or the feeds disagree |

Gates (PostHog project 56041, `spaces_app` events of the new version against the
previous stable, the most active non-prerelease version the week before the stage):

- adoption: at least 15 installs of the new version sent `cua_app_active` since the stage began;
- space create: `cua_space_create` error share at most 10 points above the previous version's, at least 30 events each;
- update: failed share of `cua_app_update` from the previous version (the one installing the new one) at most 15 points above its share the week before, at least 20 attempts;
- incident: no active incident.io incident with "spaces" in its name (needs the `INCIDENT_IO_API_KEY` secret; skipped and said so in the summary until it is set).
- crash: Sentry release health (org and project in `config/spaces-crash-reporting.json`, release `cua-spaces-desktop@<version>`, environment `stable`). Crash-free sessions of the new version since the stage began must be at most 0.5 points below the previous version's over the same window, with at least 100 sessions on the new version (fewer: wait). A previous version with fewer than 100 sessions (it shipped before crash reporting) is no baseline, so the new version must reach 99% crash-free. Needs the `SENTRY_AUTH_TOKEN` secret (`org:read`, `project:read`); skipped and said so in the summary until it is set. Crash reports and sessions only come from people who left "Share usage data" on, so the numbers are a sample.

Thresholds are flags of that script. Secrets: `POSTHOG_API_KEY` (query read),
optional `INCIDENT_IO_API_KEY`, `SENTRY_AUTH_TOKEN`, and the existing
`ALERTMANAGER_INTAKE_TOKEN`.

Dry run: **Actions > Release: auto rollout (Cua Spaces) > Run workflow** (dry run
is on by default for a manual run). It prints the decision and every gate in the
job summary and dispatches, pauses and alerts nothing. Scheduled runs act. Locally:
`POSTHOG_API_KEY=... python3 .github/scripts/release_rollout_auto.py --feed-dir <dir>`
prints the same decision as JSON.

## What each stage does

| Stage | electron-updater (Windows, Linux) | Sparkle (macOS) |
|---|---|---|
| `10` | `stagingPercentage: 10` | newest stable item gets `pubDate` = now and a 10-year `phasedRolloutInterval`: group 0 of 7 only, 14.3% |
| `50` | `stagingPercentage: 50` | `pubDate` = 3 weeks ago, interval 1 week: groups 0 to 3, 57.1%. Group 4 follows a week later |
| `100` | field removed | interval removed |
| `pause` | `stagingPercentage: 0` | item removed from the appcast (kept in `cua-spaces-appcast-paused.xml`) |

Resuming (`10`, `50`, `100` after `pause`) restores the paused Sparkle item
first. Installs that already took the update keep it in every case: neither
updater downgrades.

## Research (primary sources)

### electron-updater `stagingPercentage` (we ship 6.8.9)

Source: `AppUpdater.ts` in electron-builder, `isStagingMatch` and
`getOrCreateStagingUserId`.

- Each install makes one random UUID the first time it needs it and stores it
  in `<userData>/.updaterId`. It is stable across launches and updates, and
  lost only if the user data folder is deleted (a reinstall that keeps the
  folder keeps the id).
- The check: take the last 4 bytes of the UUID as a number, divide by
  `0xffffffff` to get a fixed position `p` in [0, 1), and the install is
  offered the update when `p < stagingPercentage / 100`. So the groups are
  nested: the 10% that got it stay inside the 50%, and 100% adds the rest. A
  missing or non-numeric `stagingPercentage` means everyone.
- It is evaluated on every check, so lowering the number later stops offering
  the update to installs that have not downloaded it. `0` offers it to nobody.
- The updater sends the id as an `x-user-staging-id` request header, so a
  server could use it. GitHub release downloads ignore it.
- Pause and rollback: nothing is undone on installs that already updated, and
  `allowDowngrade` is false in our updater (`src/updater.ts`). Rolling back
  means shipping a newer fixed version. An update already downloaded and
  waiting for `autoInstallOnAppQuit` is not cancelled by a later feed change.
- electron-builder can write `stagingPercentage` itself, but we edit the
  uploaded feed text instead, so the percentage changes without a rebuild.
- Choice for pause: the design note said "restore the previous feed file".
  Nothing keeps previous feed files today, and `stagingPercentage: 0` has the
  same effect for clients without needing one, and resumes cleanly.

### Sparkle phased rollout (we ship 2.10.0, `Package.swift`)

Sources: sparkle-project.org publishing docs ("Phased group rollouts"),
`SUPhasedUpdateGroupInfo.m`, `SUAppcastDriver.m` (`itemIsReadyForPhasedRollout`),
`SUAppcastItem.h` (needs Sparkle 1.25 or later, so 2.10.0 supports it).

- An item needs a `pubDate` and `<sparkle:phasedRolloutInterval>` (seconds).
- Sparkle hardcodes 7 groups. Each install stores a random number in user
  defaults (`SUUpdateGroupIdentifier`) and its group is that number mod 7. The
  id is not sent to a server and is re-generated after an update is
  downloaded.
- Group `k` is offered the item once `now >= pubDate + k * interval`. So the
  share offered over time is 1/7, 2/7 ... 7/7 (14%, 29%, 43%, 57%, 71%, 86%,
  100%), by time, not by a number we pick.
- Phased rollout does not apply to critical updates, nor when the user
  checks manually (Check for Updates...): those see the update at once.
- Pause: removing the item from the appcast. Sparkle cannot downgrade, so
  installs that updated stay. Rollback is a fixed higher-numbered release.
- Our appcast has no feed-level signature (only per-item EdDSA on the disk
  image), so editing the XML needs no re-signing.

**Matching 10/50/100.** Not exactly, but closely, by choosing the interval and
`pubDate` ourselves per stage instead of leaving the clock to run: a huge
interval pins the offer to group 0 (14.3%, the "10%" stage); an interval of one
week with `pubDate` three weeks back opens groups 0 to 3 (57.1%, the "50%"
stage); removing the interval is 100%. The limits are that 14% is not 10%, and
that the 50% stage is only held for a week: Sparkle opens group 4 after that
unless someone advances or pauses. Manual update checks bypass the stages.

**Recommended settings:** electron-updater 10, 50, 100 as is. Sparkle through
this workflow's mapping above (about 14%, 57%, 100%); a 7-day gate between
stages is the default cadence for macOS, and the 50% stage should not be left
unattended past a week. Accept the difference, no custom update server.

## Rehearsal on the beta feed

Beta stays at 100% in production. To rehearse without touching stable, use
`channel: beta`; it only edits `beta*.yml` and the newest `beta` appcast item,
which only apps on "Update to: Beta" read.

1. Dry run first: `channel: beta`, `platform: all`, `stage: 10`,
   `dry_run: true`. Read the diff in the job summary (expect
   `stagingPercentage: 10` in `beta*.yml`, one `phasedRolloutInterval` on the
   newest beta item).
2. Run it for real (`dry_run: false`). On a test install of the beta app on
   Windows or Linux, set the Update channel to Beta, then
   `curl -L https://github.com/trycua/cua/releases/download/cua-spaces-latest/beta.yml`
   shows the field; the updater log prints `Staging percentage: 0.1, percentage: ...`.
3. `stage: pause`, check the beta appcast item is gone and `stagingPercentage: 0`
   is set, then `stage: 100` and check the item is back with no interval.
4. Always end on `stage: 100` for `channel: beta` so beta is back to everyone.
   The beta pause sidecar shares the name `cua-spaces-appcast-paused.xml` with
   stable, so do not have both channels paused at once.
5. Do not run the workflow on stable until there is a stable release to stage.
