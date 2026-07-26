# Cua RFC process

Requests for comments (RFCs) are the feedback and decision mechanism for
changes that should be understood before they ship. Cua keeps RFCs in this
repository so the proposal, implementation, tests, and history can be reviewed
together.

RFC drafts are decision inputs. Once an RFC is accepted, the merged RFC and
linked implementation pull requests become the source of truth.

## When to use an RFC

Use an RFC for changes that affect one or more of the following:

- public SDK, CLI, MCP, protocol, or file-format contracts;
- cross-platform architecture or process and permission boundaries;
- security, privacy, trust, or authorization behavior;
- compatibility, migration, or deprecation policy;
- behavior shared by multiple Cua components; or
- decisions where external contributor feedback would materially improve the
  outcome.

An RFC is not required for small bug fixes, routine documentation changes,
internal refactors that preserve public behavior, or urgent private security
response. Suspected vulnerabilities must use the repository's private security
reporting path instead of a public RFC.

## Repository structure

```text
rfcs/
├── 0000-template.md
├── <issue>-<short-name>.md
└── <issue>/
    ├── architecture-overview.png
    └── other-supporting-material...
```

The GitHub discussion issue number is the RFC identifier. For example, issue
`#2447` owns `rfcs/2447-short-name.md` and the optional `rfcs/2447/` sidecar
directory. Relative asset links keep the proposal portable and reviewable on
GitHub.

## Lifecycle

1. Open an issue using the **Request for comments** issue form. The issue is the
   discussion and decision record.
2. For a proposal that needs more than the issue body, copy
   [`0000-template.md`](0000-template.md) to `<issue>-<short-name>.md`, fill in
   the issue URL, and open a pull request with `status: review`.
3. Keep the issue and RFC pull request linked in both directions. Review the
   public contract, alternatives, migration, platform behavior, security, and
   unresolved questions before implementation begins.
4. A maintainer posts a decision summary in the issue covering material
   feedback, accepted changes, rejected alternatives, remaining risks, and the
   final disposition.
5. For an accepted proposal, set `status: accepted` and merge the RFC before or
   with the first implementation pull request. Link each implementation issue
   and pull request from the RFC.
6. Set the RFC to `completed` when its acceptance criteria ship. Use
   `declined`, `withdrawn`, or `superseded` when appropriate; never rewrite a
   historical decision to make it look as though another outcome was chosen.

Normal proposals should receive a review period proportionate to their impact.
Breaking public contracts and cross-platform permission changes should normally
remain open for at least seven calendar days. If an urgent change needs a
shorter window, record the reason in the decision summary.

## Status vocabulary

| Status | Meaning |
| --- | --- |
| `draft` | The author is still shaping the proposal. |
| `review` | The proposal is ready for technical and product feedback. |
| `accepted` | The decision is approved; implementation may still be pending. |
| `completed` | The accepted proposal's required implementation has shipped. |
| `declined` | Review concluded that Cua should not adopt the proposal. |
| `withdrawn` | The author stopped the proposal before a decision. |
| `superseded` | A linked later RFC replaces this decision. |

## Review expectations

Review the proposal rather than only the implementation plan. In particular,
check:

- whether the user problem and success criteria are concrete;
- which layer owns the contract and whether another standard already owns it;
- lifecycle, cancellation, concurrency, and failure isolation;
- macOS permission identity, Windows session placement, and Linux display
  constraints when desktop behavior is involved;
- compatibility and an observable rollback path;
- privacy boundaries and telemetry content; and
- how parity will be proven across public surfaces.

Public RFCs must not contain credentials, customer or partner identities,
private reports, personal data, sensitive screenshots, raw session transcripts,
or exploit-enabling security details.

## Labels

The issue form applies the repository's existing `review` label and prefixes the
title with `[RFC]`, so it works without provisioning new repository state. If
RFC volume later warrants dedicated labels, add `type: rfc` plus lifecycle
labels for `accepted`, `declined`, `withdrawn`, and `superseded`. The RFC
frontmatter and maintainer decision summary remain the authoritative status.
