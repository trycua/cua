---
title: Ordered, attachment-aware live traces for Cua Bench
authors:
  - HaniLabs
created: 2026-10-21
last_updated: 2026-10-21
status: review
discussion: https://github.com/trycua/cua/issues/5685
rfc_pr:
implementation:
  - https://github.com/trycua/cua/pull/5695
supersedes:
superseded_by:
---

# RFC: Ordered, attachment-aware live traces for Cua Bench

## Summary

Extend Cua Bench with opt-in versioned trace bundles, ordered live viewing,
managed attachments, disk-only runner capture and navigation through explicitly
correlated evidence. Keep legacy dataset recording and exports unchanged by
default. This document describes a proposed contract and its local experiment;
no maintainer acceptance is recorded. The experiment in draft PR #5695 is a
review input, not a decision or a released API.

## Motivation

External collectors deliver observations late and use different clocks. A task
can produce evidence too large for event JSON or browser memory. Users need to
inspect that evidence during a run, identify related actions and requests, and
retain a complete recording without accumulating legacy rows in memory.

The original saved-dataset viewer loads a completed dataset. Sorting its DOM
rows alone does not preserve image selection, overlays or navigation. Arbitrary
payload timestamps and matching filenames cannot establish chronology or cause.

## Goals

- Inspect committed events while recording continues, with bounded pages.
- Choose arrival or aligned observation-time ordering without silently comparing
  unrelated source clocks.
- Preserve event identity, selected images and expanded data across late arrivals,
  sorting, pagination, pause/resume and reconnect.
- Import managed files, preview bounded inert content, and download large evidence
  without a whole-file JavaScript Blob.
- Offer an explicit disk-only runner mode and refuse incomplete legacy exports.
- Link evidence using supplied identifiers and provenance, without causal inference.
- Preserve default recorder calls, dataset schema and runner outputs.

## Non-goals

Native OS, network or device collectors; Android services; account orchestration;
operation discovery; Go workflow generation or execution; automatic clock
synchronization; remote hosting; ingestion endpoints; replay of captured content;
and concurrent writers to one bundle are outside this proposal.

## Terminology

- **Generation:** immutable bundle identity. An event identity is generation plus
  arrival sequence, independent of presentation order.
- **Trajectory:** the existing recorder session ID. One bundle may contain several.
- **Committed event:** a complete newline-terminated journal record whose managed
  attachments have already been published.
- **Aligned time:** a collector-supplied UTC timestamp with a declared source clock.
  The viewer does not verify the mapping or infer one.
- **Correlation:** an exact supplied namespace, kind, ID and provenance tuple,
  scoped to one trajectory. A match is a navigation link, not proof of cause.

## Current state

Upstream baseline inspected: `fca21937bc4e1420a23d544d11dbdf987f0266e6`.
[Tracing](../../libs/cua-bench/cua_bench/tracing.py) retains rows and exports the
five legacy columns: `event_name`, `data_json`, `data_images`, `trajectory_id`,
`timestamp`. [PR #5687](https://github.com/trycua/cua/pull/5687) demonstrates that
existing functionality with a synthetic JSONL collector.

The experimental [PR #5695](https://github.com/trycua/cua/pull/5695) includes that
contribution and its original history. Its handoff candidate was
`6a875c92788f99909227cebb0201ac97303d78f8`. Local follow-up work adds the three
extensions below and the shared collector encoding correction. Earlier RFC
comment text saying no implementation PR exists is stale; #5695 is open and draft.

The implementation lives in
[trace_bundle.py](../../libs/cua-bench/cua_bench/trace_bundle.py),
[trace_live_viewer.py](../../libs/cua-bench/cua_bench/trace_live_viewer.py), the
[viewer](../../libs/cua-bench/cua_bench/www/trace_live.js), and the existing
[episode runner](../../libs/cua-bench/cua_bench/runner/episode.py).

## Proposal

### Ownership and public surface

Bench owns persistence and inspection. Collectors obtain permissions, collect and
redact evidence, and submit observations to one serialized recorder owner. No
viewer route accepts collector writes or imports a path supplied by event data.

| Surface                                                                   | Proposed contract implemented in the experiment                                                  |
| ------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `TraceBundleWriter(directory, max_attachment=100*1024**2, quota=1024**3)` | Exclusively create a new bundle; context manager finalizes it.                                   |
| `writer.add_attachment(path, name=None, media_type=...)`                  | Copy a regular non-symlink file and return a writer-scoped descriptor.                           |
| `Tracing(env, bundle_writer=None, retain_rows=True)`                      | Existing default recorder; disk-only requires a writer.                                          |
| `trace.record(name, data, data_images=None, context=None, attachments=None)`   | Existing positional arguments and return value; new arguments are keyword-only.                  |
| `trace.bundle(path, retain_rows=True, **writer_options)`                  | Temporarily attach a writer, forward its limits, finalize and restore recorder settings on exit. |
| `cb interact TASK --trace-bundle PATH`                                    | Bundle-only unless `--trace-out` also requests legacy rows/export.                               |
| `cb run TASK_OR_DATASET --live-trace`                                     | Add a per-episode bundle while retaining dataset and ATIF exports.                               |
| `cb run TASK_OR_DATASET --trace-only`                                     | Per-episode bundle with no retained rows, legacy dataset or ATIF export.                         |
| `cb trace view PATH --follow --max-download-mib N`                        | Follow a bundle; independently authorize downloads up to N MiB, default 100, range 1–4096.       |

Existing recorder calls before `start()` remain ignored. `start()` begins a new
trajectory and clears retained rows but never truncates a bundle or reuses journal
sequences. Callers must serialize recording; no multi-process writer is promised.

### Bundle format and clocks

Version 1 uses `trace.json`, `events.jsonl`, and `artifacts/<opaque-id>`. Manifest
fields are `format: cua-bench-trace`, `version: 1`, `generation`, `state`,
`final_sequence`, and additive `created_at`, `clock` and `limits` descriptions.
Old experimental version-1 bundles without those descriptions remain readable.
Creation of the directory is exclusive; existing directories are never taken over.

Each event contains `sequence`, `trajectory_id`, `event_name`, `recorded_at`,
`context`, `data`, `images`, and `attachments`. Sequence starts at zero.
`recorded_at` is the recorder UTC wall time; sequence remains authoritative if
that clock moves backwards. Original payload data stays under `data`.

`context.aligned_at` is an optional ISO timestamp with an explicit timezone,
normalized to UTC milliseconds. It requires `context.clock_domain`. Raw source
values such as `source_timestamp_ms`, producer IDs, source sequences, alignment
method and uncertainty can be supplied in context and are displayed as evidence;
they do not acquire sorting semantics. The current minimum provenance is the
clock domain. A stronger standardized mapping/uncertainty schema remains a review
question; this experiment does not claim automatic synchronization or verified
alignment.

Arrival order is the default. Observation order compares aligned UTC timestamps,
with sequence as the tie-breaker. Unaligned events follow, grouped lexically by
clock domain and then arrival sequence. Timestamp proximity creates no links.
Image navigation follows the selected ordering across the bundle; it is not
restricted to one trajectory or clock group.

The writer copies and hashes attachment bytes in 64 KiB chunks and atomically
publishes each file before appending its referring event. Each append flushes a
complete line; close fsyncs the journal and atomically publishes terminal state.
Visibility to local readers does not imply power-loss durability before close.
A partial trailing line is withheld. A complete corrupt line, journal truncation,
replacement or generation change refuses explicitly. Unknown versions refuse.
Opening a recording never migrates, repairs or deletes it.

### Lifecycle, disk-only capture and export compatibility

Writer state is `recording`, `completed`, or `failed`. Successful close records
`final_sequence`; repeated close is harmless and subsequent writes refuse.
Exceptional scope exits, including cancellation, mark failure. Abrupt termination
leaves an unfinalized bundle; its complete prefix remains inspectable. Completion
requires the reader to have all bytes through the advertised final sequence.

`AgentOptions(trace_only=True)` and `--trace-only` select disk-only capture.
They imply a bundle and explicitly omit both legacy export attempts. Python
runner options `save_pngs` and `filter_events` conflict with this mode and refuse
before task execution. Task results and agent log files still work. Successful episode result metadata
records the bundle path, final trace state and `legacy_exports: false`. Recorder
settings are restored after success, failure and cancellation. Recording failures
remain best-effort with respect to task execution and mark the bundle failed;
this is not a guarantee that every task result has a complete trace.

A trajectory that omitted any rows cannot later export an apparently complete
legacy dataset, including separate-PNG export, after leaving its bundle scope.
Export refuses until a fresh `start()`. There is no automatic replay into memory
or implicit bundle-to-dataset conversion. Existing fully retained exports keep
their schema; context and generic attachments are a documented lossy projection.

The writer's descriptor registry and reader's ordering/correlation indexes are
separate disposable SQLite files. The JSONL journal remains authoritative. Each
SQLite connection has a 2 MiB page cache; Python does not keep a descriptor per
attachment or a row per event in disk-only mode. This bounds trace retention,
not task/agent memory, an individual screenshot's encoding allocation, operating
system file cache or total process RSS. Runner bundles retain the default 1 GiB
quota. Python writer/bundle integrations can configure a larger quota explicitly.

### Limits, integrity and downloads

| Resource                      | Contract                                                                 |
| ----------------------------- | ------------------------------------------------------------------------ |
| Encoded event                 | 1 MiB including terminating newline                                      |
| Compatibility batch API       | At most 500 events, approximately 2 MiB of event JSON                    |
| Main or related page          | 100 bounded event summaries                                              |
| Imported attachment           | Default 100 MiB; configurable up to hard ceiling 4 GiB                   |
| Bundle bytes                  | Default 1 GiB; configurable positive quota for journal and imports       |
| Viewer download               | Default 100 MiB independently; configurable up to 4 GiB                  |
| Active transfers              | At most two per viewer; additional requests receive HTTP 429             |
| Pending download capabilities | At most 16; single use; expire after 60 seconds                          |
| Transfer working memory       | 64 KiB copy chunks; private verified file snapshots on disk              |
| Temporary transfer disk       | Up to two attachments at the configured viewer limit                     |
| Text/JSON preview response    | At most 256 KiB, escaped as text                                         |
| Image preview                 | At most 8 MiB compressed bytes; PNG, JPEG, GIF or WebP only              |
| ARIA overlays                 | At most 1,000 inert bounds per selected event                            |
| Correlations                  | At most 32 per event; four nonempty strings, at most 256 characters each |

Limits fail explicitly rather than silently dropping requested evidence. Imported
but unreferenced files can consume quota; version 1 does not garbage-collect live
bundles. Temporary indexes and their metadata are additional disk overhead.

Descriptors contain `id`, `generation`, `name`, `media_type`, `size`, and `sha256`.
Only registered IDs can be served. Foreign descriptors, traversal, symlinks,
special files, missing files and size/digest mismatches refuse. The server copies
an indexed file to a private temporary snapshot and verifies its digest before
sending bytes. Later source-file mutation cannot change the validated download.
Local attachment directories must remain controlled by the recording account;
this is not isolation from a hostile process with the same filesystem privileges.

The browser obtains a short-lived single-use download URL using its bearer
capability, then uses a normal download link. The browser download manager writes
the response without whole-file JavaScript buffering. This is portable without a
File System Access API requirement. The URL authorizes one managed attachment,
never an arbitrary path. No range/resume support is offered; failed, expired or
busy transfers need a new click. Hash verification delays time to first byte.

Previews use a separate authenticated endpoint and bounded responses. HTML, SVG,
scripts and other active/binary formats are download-only. HTTP responses always
force attachment disposition and octet-stream with nosniff; permitted image
previews use local blobs and text previews use text content. Preview byte limits
do not claim to bound a browser decoder's pixel allocations.

### Related evidence and viewer state

`context.correlations` is a list of objects with exactly `namespace`, `kind`,
`id`, and `provenance` string fields. For example, a redacted network request and
a platform trace event may both supply namespace `checkout`, kind `request`,
ID `request-1`, and provenance `collector-session-1`. An action may carry that
request tuple plus its own action tuple to bridge evidence deliberately.

Related evidence matches all four fields within one trajectory and generation.
No arbitrary payload key is promoted into a relationship. Provenance is the
collector's assertion, displayed in context, not authenticated attribution or
causal proof. Artifact events expose their existing managed attachments.

Related pages use the same ordering and sequence watermark as main pages.
Following a link selects by sequence and locates its page; late arrivals and
sorting cannot change that identity. Pausing freezes both views at the displayed
watermark while status polling continues. Resuming catches up. Reconnect retains
selection; generation changes require an explicit reopen. Expanded data and an
open image remain associated with their selected identities.

### Local viewer and platform contract

The server binds to loopback with a fresh bearer capability bootstrapped through
a URL fragment, then cleared from browser history. Validate Host and Origin;
private JSON/preview routes require the header. Download URLs use the bounded
single-use exception above. Disable caching, referrers and request logging, and
serve only local UI assets under a restrictive CSP. No trace data enters telemetry.

Polling is every 500 ms in follow mode. Status polling checks the journal's
incremental disk index; only one page and selected event details reach the UI.
This implementation retries at that fixed interval rather than promising an
unimplemented backoff scheme. Without `--follow`, a fixed snapshot cutoff applies.
Legacy dataset `--follow` refuses. Closing the browser/server does not stop the
writer. The server releases reader resources when it exits.

Windows, macOS and Linux share filesystem/loopback behavior; no host input or
capture permission is added. Android evidence comes from host-side collectors.
Network filesystems are outside the live-writing guarantee. Native OS collectors
and Cua Driver's desktop matrix are not part of Bench-only acceptance.

## Alternatives considered

- Repeated dataset reloads cannot expose unsaved events or guarantee coherent reads.
- Payload timestamp guessing makes unsupported clock assumptions.
- Base64 attachments inflate the journal and browser memory.
- SQLite as the authoritative bundle offers transactional advantages but loses
  simple JSONL inspection. JSONL plus disposable indexes is the experiment, not
  a final maintainer decision.
- SSE/WebSockets may reduce latency, but bounded polling is sufficient to test the
  local target and avoids a second replay protocol.
- File System Access downloads need browser-specific support. A native browser
  download with a narrow capability works across the supported browser harnesses.
- Streaming legacy/ATIF export could preserve both outputs in disk-only mode,
  but requires a separate compatibility and memory contract. Explicit omission
  and export refusal avoid silently partial output now.
- Timestamp-derived relationships would suggest causality the evidence does not prove.

## Compatibility and migration

All writing stays opt-in. Existing datasets remain readable with their existing
viewer; older viewers use the legacy projection. No schema migration occurs on
open. Rollback disables bundle capture and uses ordinary dataset recording.
Keep bundles rather than deleting evidence. Public contract adoption requires
the recorded RFC decision, followed by reader/fixture and implementation review.
Use feature-appropriate PR/release titles for user-visible production additions.

## Security, privacy, and telemetry

Collectors own permission and redaction before persistence. Examples and tests
use synthetic data. The viewer neither acquires new capture permissions nor
executes captured scripts, HTML or requests. Managed IDs, integrity verification,
limits and inert content handling define the proposed local read boundary. These
are design requirements, not a report of a vulnerability in shipped code.
No payloads, filenames, source URLs, capabilities or captured content are logged
or added to telemetry.

## Implementation plan

1. Review this document and record a maintainer decision in #5685. Merge an accepted
   RFC before or with accepted implementation; the local experiment remains draft.
2. Land the independent JSONL encoding correction in #5687 and carry the same
   original-author commit into #5695. Do not cite a PR as its own salvage source.
3. Review reader/writer, legacy compatibility, runner lifecycle, download boundary
   and explicit correlation changes in #5695. Preserve the source contribution.
4. Certify a stable candidate with the Bench suite, browser, scale, package and
   documentation checks. Run Windows, Linux and macOS checks on that candidate.
5. After eventual merge, run the relevant main-branch smoke and release-path check.
   This goal does not authorize merge or release.

## Test and acceptance plan

Require subprocess coverage under non-UTF-8 locales; unchanged legacy export
schema goldens; disk-only success, failure, cancellation and post-scope refusal;
record/write failures; invalid contexts; unknown versions; incomplete/corrupt
journals; generation changes; foreign/symlink/modified attachments; quota and
viewer-limit refusal; ticket authorization, expiry, single-use and admission
bounds; real browser downloads above 100 MiB with byte/digest verification;
related evidence across pages, sorts, late arrivals, pause/resume and reconnect;
and unrelated clocks, trajectories and provenance that must not match.

Run actual Chromium with external requests blocked. Keep the 100,000-event
fixture at 100 rendered rows, under 16 MiB traced Python allocations and 32 MiB
JavaScript heap; these exclude runtime baselines and SQLite native cache. Add a
producer fixture with many attachments to cover registry retention. Measure the
two-second local append-to-display target after indexing under the fixture load.
Test all relevant host filesystems and loopback behavior on the final candidate.
A configured CI lane or earlier passing SHA does not certify a later change.

The handoff candidate's focused Linux, Windows and macOS Intel results are
historical evidence only. New extension results belong in the current PR
execution record with the exact tested SHA, commands and limitations. Hosted
macOS ARM64 is not covered by the Intel lane.

## Unresolved questions

1. Accept JSONL with disposable SQLite indexes, or choose a different bundle store?
2. Accept the proposed API/CLI names, limits, quota and explicit no-export mode?
3. Require standardized alignment provenance/uncertainty beyond a clock domain?
4. Accept exact supplied correlation tuples as the initial navigation contract?
5. Accept temporary disk snapshots and single-use URLs for native downloads?
6. Require a separately reviewed streaming legacy/ATIF export in a later version?

## Decision record

As verified on October 21, 2026, #5685 is open with no recorded maintainer decision.
PR #5695 exists and is draft; #5687 is the separate example contribution. This
RFC is a review proposal. The user authorized continuing the local experiment
while upstream acceptance remains gated. No accepted alternatives, waived risks,
merge approval, released version or RFC completion is asserted here.
