# Cua Driver Bench

Cua Driver Bench measures how a complete computer-use agent operates desktop
applications through Cua Driver. It separates task outcome, driver
participation, apparatus certification, and comparison eligibility so that a
local diagnostic result cannot be mistaken for certified benchmark evidence.

This public directory contains the benchmark definition, experiment profiles,
calibration framework, conformance fixtures, and documentation. The reusable
runtime lives in [`../../libs/cua-bench-runtime`](../../libs/cua-bench-runtime).

The held-out task pack is intentionally not part of this repository. Supply an
authorized task root when running the benchmark; see [`tasks/README.md`](tasks/README.md).

## Import status

This is a sanitized snapshot import from Cua Driver Bench. The source revision,
license review, exclusions, and contributor credit are recorded in
[`PROVENANCE.md`](PROVENANCE.md).

The initial pull request remains a draft while runtime packaging, automated
evaluation, documentation links, and CI are adapted to the monorepo.

