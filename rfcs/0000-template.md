---
title: <Title>
authors:
  - <GitHub handle or team>
created: <YYYY-MM-DD>
last_updated: <YYYY-MM-DD>
status: draft
discussion: <GitHub issue URL>
rfc_pr:
implementation:
  - <GitHub issue or pull request URL>
supersedes:
superseded_by:
---

# RFC: <Title>

## Summary

One paragraph describing the proposed decision.

## Motivation

What user or engineering problem requires a decision, and why now?

## Goals

- A concrete outcome this proposal must achieve.

## Non-goals

- A nearby concern this proposal deliberately does not solve.

## Terminology

Define terms whose meaning affects the proposal.

## Current state

Describe the observable architecture or behavior today. Link repository
evidence where possible.

## Proposal

Describe the public contract, component ownership, data and control flow,
lifecycle, compatibility, and platform behavior.

Place supporting assets in the sibling `<issue>/` directory and reference them
with relative links:

```md
![Architecture overview](<issue>/architecture-overview.png)
```

## Alternatives considered

Explain the strongest alternatives and why they were not selected.

## Compatibility and migration

Describe additive delivery, deprecations, breaking changes, rollback, and
release sequencing.

## Security, privacy, and telemetry

State permission boundaries, sensitive-data handling, and what telemetry may or
may not contain.

## Implementation plan

Split the work into independently reviewable increments with explicit parity
and rollback gates.

## Test and acceptance plan

List the evidence required to consider the RFC implemented.

## Unresolved questions

- A decision that must be made during review.

## Decision record

Complete this section when review concludes. Summarize material feedback,
accepted changes, rejected alternatives, remaining risks, and final disposition.
