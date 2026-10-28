---
rfc: 2969
title: 'Cua Driver: Compact-by-Default Agent Payloads'
authors:
  - Cua maintainers
created: 2026-08-07
last_updated: 2026-08-07
status: review
discussion: https://github.com/trycua/cua/issues/2969
rfc_pr: https://github.com/trycua/cua/pull/2970
implementation:
supersedes:
superseded_by:
---

# RFC 2969: Cua Driver: Compact-by-Default Agent Payloads

## Summary

Cua Driver will make its agent-facing payloads compact by default so a small
local model can drive the same contract a frontier model drives. The proposal
adds no tool parameter, no CLI flag, and no environment variable.

The work is two tiers, and the tiers are not equally negotiable.

Tier 1 is a set of unconditional reductions. Every one of them removes bytes
that no caller wanted, or repairs a contract defect that a caller of any size
can hit. They need no negotiation because there is no caller for whom the
current behavior is better: collapse duplicate semantic refs that share one
accessible name, canonicalize and shorten the tool-schema prefix, apply the
operator's existing permission-policy deny list to `tools/list`, and fix two
small contract defects that produced observed local-model failures.

Tier 2 is capability negotiation, reserved for choices that are genuinely
model-dependent rather than merely wasteful. MCP's `initialize` handshake is
the principled surface for those, and Cua Driver already parses the client's
declared capabilities there before discarding them. The only clearly
model-dependent choice today is whether the client can accept images at all.

This RFC deliberately does not propose a negotiated context budget. The
failures measured so far are not overflow failures, and this document states
what evidence would change that.

## Motivation

Cua Driver's tool surface grew to serve agents that can afford it. A frontier
model with a large context window and cached prefixes absorbs a 126 KB tool
prefix and a four-times-redundant page snapshot without visible cost. That
absorption hid the growth rather than justifying it.

Running the same contract against a model on the user's own machine removes
the cushion and makes the cost legible.

The following numbers were measured on 2026-08-07 on an Apple M5 Max running
macOS 26.5.2, with `qwen3.6:35b-a3b-mtp-q8_0` served by Ollama 0.32.5 and
driven through opencode 1.18.14 against `cua-driver` 0.19.0.

**The tool prefix dominates session startup.** The first inference of every
session costs 2m41s to 2m44s, essentially all of it prompt-eval over the tool
schemas. Once the prefix is cached, subsequent steps run in 3 to 80 seconds.
Every new session pays the full cost again. A live capture of the macOS
`tools/list` response is 126,461 bytes across 54 tools — roughly 34,000 tokens
at a typical 3.7 bytes per token.

**Snapshots repeat themselves.** `get_browser_state` with
`snapshot_format: "semantic_v2"` on a YouTube search-results page returned 113
nodes for roughly 25 videos. Each result yielded four refs carrying the same
accessible name under the roles `heading`, `link`, `generic`, and
`statictext`. Three of the four are wrong to click. The redundancy is
therefore not only a payload cost; it is a four-way choice the model has to
get right on every result, with no signal in the snapshot to distinguish the
correct ref from three decoys.

**The observed failures were not context overflow.** Two failure modes
appeared, and neither is a budget problem:

- The model fabricated `snapshot_id` and `element_token` handles rather than
  reusing the handles it had just been given.
- The model read a successful `browser_prepare` as a refusal and abandoned the
  accessibility route.

Both point at contract legibility, not context size. Both are fixable without
negotiation, and both fixes make the contract better for a frontier model too.

The temptation at this point is to add a `compact: true` argument and move on.
That would be the wrong decision, and this RFC treats it as such: a new
parameter costs schema bytes on every tool that carries it, and costs the
model a decision on every call it makes, in exchange for making the default
behavior no better than it is today.

## Goals

- Reduce the serialized `tools/list` response materially without removing a
  tool, a parameter, or a schema field a client depends on.
- Emit one ref per addressable thing in `semantic_v2`, rather than one ref per
  accessibility role that happens to carry the same accessible name.
- Let an operator's existing `deny.tools` policy also remove those tools from
  `tools/list`, so tools that were already declared unusable stop costing
  prompt bytes.
- Make a fabricated `element_token` rejectable by a strict client before it
  reaches the driver.
- Make `browser_prepare` success unambiguous in its leading text.
- Converge the cross-platform drift in `get_window_state` snapshot defaults
  onto one documented default.
- Add zero new tool parameters, CLI flags, or environment variables.
- Keep every reduction verifiable by a captured byte count or a fixture test,
  not by assertion.

## Non-goals

- Add a `compact`, `verbose`, `budget`, `detail_level`, or equivalent argument
  to any tool.
- Add a CLI flag or environment variable that selects payload size.
- Introduce a negotiated context budget in this RFC.
- Change where permission enforcement happens. Listing behavior is
  presentational; the native registry boundary remains the enforcement point.
- Redesign the `semantic_v2` contract, its ref namespace, its continuation
  model, or its omission accounting.
- Remove `outputSchema` from `tools/list`.
- Change the desktop accessibility snapshot's shape before its redundancy has
  been measured the way the browser snapshot's was.
- Downscale `get_desktop_state`, whose full-resolution capture is a deliberate
  requirement of the desktop-scope coordinate contract.
- Change the installable skill files' role as the home for long-form workflow
  guidance.

## Terminology

**Tool prefix**
: The serialized `tools/list` response a client places in the model's context
before the first inference of a session. Its cost is paid once per session and
recurs on every new session.

**Unconditional reduction**
: A change that removes payload or repairs a defect for every caller, with no
caller for whom the previous behavior was preferable. It requires no
negotiation because there is nothing to negotiate.

**Model-dependent choice**
: A choice whose correct answer differs by client — for example, whether the
client can accept an image at all. Only these belong in negotiation.

**Ref cluster**
: A set of nodes in one `semantic_v2` snapshot that share a normalized
accessible name within a bounded region of the tree, of which at most one is
the node a caller should act on.

**Opt-up**
: A parameter that already ships and lets a caller request more than the
default. `max_elements`, `query`, `scope_ref`, and `include_screenshot` are
opt-ups. This RFC adds none and removes none.

## Current state

Everything in this section ships today. The proposal builds on it rather than
restating it.

### The tool prefix is 126 KB and unfiltered

[`ToolRegistry::tools_list`](../libs/cua-driver/rust/crates/cua-driver-core/src/tool.rs)
maps over the registration order verbatim. There is no filtering, no profile,
and no client-dependent selection. The macOS registry holds 54 tools; Windows
holds 55 and Linux 58.

A live capture — `cua-driver mcp` fed an `initialize` and a `tools/list` over
stdin — measures the macOS response at 126,461 bytes compact. Its composition:

| Component | Bytes | Note |
| --- | ---: | --- |
| Input schemas | 48,051 | 12,304 of these bytes are byte-identical repeated property definitions |
| Tool descriptions | 35,771 | |
| Output schemas | 24,714 | 30 tools; 15 carry the same 1,093-byte action-result schema |
| Annotations | 4,830 | four boolean hints per tool |
| Risk metadata | 4,345 | includes a `"version": "1"` field repeated 54 times |
| Capabilities | 2,313 | |

The largest single entries are `click` at 7,139 bytes, `type_text` at 5,938,
`get_window_state` at 5,904, `scroll` at 5,028, and `press_key` at 4,848. The
smallest is `get_config` at 398.

The repeated property definitions are the clearest waste, because they are not
even consistent with each other. `session` ships three different descriptions
across the registry: a 243-byte version on 13 tools, a 128-byte version on 7,
and a 17-byte version on 6. `element_token` repeats a 267-byte description
across 7 tools, `delivery_mode` a 428-byte description across 6.

[`tool_schema.rs`](../libs/cua-driver/rust/crates/cua-driver-core/src/tool_schema.rs)
already owns canonical builders for these shared fragments and already runs a
per-platform drift gate over them. The gate's `structural()` reducer keeps only
`type`, `enum`, and `items`, deliberately stripping prose so per-tool wording
differences do not trip it. That decision is why three `session` descriptions
coexist: the mechanism to prevent it exists and is configured not to look.

### Long-form guidance already lives out of band

[`initialize_result`](../libs/cua-driver/rust/crates/cua-driver-core/src/protocol.rs)
returns an `instructions` string deliberately kept short — measured at 1,238
bytes — with the long-form workflow pushed into the installable skill files
under `rust/Skills/cua-driver/`, fetched on demand by
[`skills.rs`](../libs/cua-driver/rust/crates/cua-driver/src/skills.rs).

The precedent is already set and already argued in the repository: eagerly
loaded prose is charged on every turn, so the detail belongs elsewhere. Tool
descriptions did not follow it. `browser_prepare` alone carries a
1,100-byte authorization essay in its description, most of which restates
`BROWSER.md`.

### `semantic_v2` already scopes, queries, and pages

[`get_browser_state`](../libs/cua-driver/rust/crates/cua-driver-core/src/browser/tools.rs)
already accepts `snapshot_format` (`dom_refs_v1` or `semantic_v2`, defaulting
to `dom_refs_v1`), `scope_ref`, `query`, `continuation`, and
`include_screenshot` (default `false`). It exposes no numeric budget
parameter; budgets are compile-time constants, and
[the semantic-state plan](../libs/cua-driver/docs/browser-semantic-state-plan.md)
states that public numeric overrides wait until performance and abuse bounds
are known.

The snapshot pipeline in
[`semantic.rs`](../libs/cua-driver/rust/crates/cua-driver-core/src/browser/semantic.rs)
already drops AX-ignored nodes and `inlinetextbox`, excludes CSS-hidden and
page-occluded nodes from the candidate set, ranks visible and actionable nodes
first, truncates text at 1,000 characters, pages at a 300-node budget, and
reports what it omitted in a structured `omitted` object with per-reason
counts.

This is a well-built snapshot. The gap is narrow and specific.

### Static-text dedup is parent-only

`remove_redundant_static_text` is the only name-based deduplication in the
pipeline. It retains every node whose role is not `statictext` or `text`, and
drops such a node only when its **immediate parent** carries an identical
name.

That rule cannot see the measured YouTube case. There the duplicates are
siblings and cousins under a shared container, and three of the four carry
roles the rule never examines. The rule fires on the vertical relationship;
the redundancy is horizontal.

Roles are otherwise emitted verbatim. There is no role folding, and `generic`
is in places synthesized by the DOM-supplement path rather than merely passed
through.

The design plan treats duplicate names as an addressing problem to be solved
by `scope_ref` and `query`. Those parameters are genuinely useful, but they
require the caller to already know which of four identically named refs is
correct. That is precisely what the model gets wrong.

### `get_window_state` already has projection parameters, with drift

`get_window_state` already accepts `query`, `max_elements`, `max_depth`,
`include_screenshot`, and `screenshot_out_file`. `query` projects both the
markdown tree and the structured elements to matching rows plus their
actionable ancestors, without renumbering `element_index`, and reports
`filtered_element_count` alongside the full `element_count`.

The defaults have drifted across platforms:

| Platform | `max_elements` | `max_depth` |
| --- | ---: | ---: |
| macOS | 2,000 | 25 |
| Windows | 5,000 | 25 |
| Linux | 5,000 | uncapped |

`include_screenshot` defaults to **`true`** on every platform, so every
`get_window_state` call ships a PNG — downscaled to a 1,568-pixel maximum
dimension — even when the caller wants only the element index.

`get_desktop_state`, by contrast, takes no limiting parameters and
deliberately captures at native resolution without downscaling, because
desktop-scope coordinates are read from it. That is correct and this RFC does
not touch it.

### Permission policy narrows authority but not the listing

Cua Driver evaluates every SDK and daemon tool call against the configured
policy stack at the native registry boundary, and SDK, CLI, MCP, and
raw-socket calls cannot bypass it. A YAML or Rego policy supplies `allow.tools`,
`allow.rules`, and `deny.tools`, and the documentation guides operators toward
restricting tool access this way.

`tools/list` ignores all of it. An operator who denies forty tools still pays
for forty tool schemas in every session's prompt prefix, and the model still
spends its attention choosing among tools it will be refused for calling.

The one flag that ever gated the roster, `--claude-code-computer-use-compat`,
is now inert for tool selection. The SDK's `RuntimeOptions` can only add host
tools, never remove them.

### `initialize` parses client capabilities and discards them

`Request::initialize_metadata` already extracts the client's `protocolVersion`,
`clientInfo`, and presence flags for `tools`, `roots`, `sampling`,
`experimental`, and elicitation, truncating each value to 128 characters and
retaining nothing. `initialize_result` returns a static `protocolVersion`,
`capabilities`, `serverInfo`, and `instructions`.

Nothing in the codebase feeds a client capability into tool selection or
response shaping. The negotiation surface exists, is already wired for
reading, and is not connected to anything.

### Handles are strict at runtime but weak in the schema

`snapshot_id` declares `"pattern": "^s[0-9a-f]{8}$"` in its shared schema
fragment. `element_token` — the handle callers are told to prefer — declares
no pattern at all, even though its format is fixed at
`s{8-hex}:{element_index}` and documented as such in
[`element_token.rs`](../libs/cua-driver/rust/crates/cua-driver-core/src/element_token.rs).

The runtime rejects fabricated handles cleanly, with distinct refusal codes
for `invalid_element_token`, `stale_element_token`, and
`generation_mismatch`, and a message directing the caller to re-snapshot. But
a strict MCP client validating arguments against the advertised schema will
happily forward an invented base64 blob, because the schema permits any
string.

### `browser_prepare` reports success in the language of refusal

On a successful isolated launch, `browser_prepare` returns
`status: "ok"`, `prepared: true`, and a populated `side_effects` object. Its
text block renders as:

```text
browser_prepare: endpoint available — Launched a separate driver-owned
isolated Chromium process; the requested browser process was not modified or
terminated.
```

The assurance about what was *not* done is correct and worth stating. Leading
with it, in the text block a model reads first, is what produced the observed
misreading. The structured fields are unambiguous; the prose is not.

## Proposal

### Tier 1: unconditional reductions

These need no negotiation. Each removes payload no caller asked for, or
repairs a defect any caller can hit.

#### 1. Collapse ref clusters that share an accessible name

Extend deduplication from the parent-only static-text rule to a cluster rule
over the candidate set.

Within one nearest common ancestor, when several nodes share a normalized
accessible name, emit one ref. Select the node that carries actions. Record
the collapsed roles on the surviving node so the snapshot stays honest about
what was folded, and keep the collapsed nodes visible in the rendered outline
where they carry structure.

Three rules bound it:

- A cluster is never collapsed to zero. If exactly one node in the cluster
  carries actions, that node survives unconditionally.
- Collapsing never crosses a frame boundary or an actionable ancestor.
- A node whose accessible name differs after normalization is never in the
  cluster, regardless of visual similarity.

`content_refs` — the actionless array — drops members whose name duplicates a
surviving actionable ref in the same cluster. A content ref that names
something no actionable ref names is retained, because it is the only way to
read that text.

This helps every model. A frontier model also mis-clicks a `generic` wrapper,
and every caller pays the four-times cost today.

#### 2. Canonicalize and shorten the tool prefix

Four changes, in descending order of measured value:

**Canonicalize shared fragment prose.** Each shared parameter ships one
description, chosen as the shortest correct one. Extend the drift gate's
`structural()` reducer to compare `description` and `pattern` in addition to
`type`, `enum`, and `items`, so a divergent description fails a test rather
than shipping to 13 tools.

**Hold descriptions to a stated budget.** Tool descriptions state what the
tool does, its required arguments, and its most common failure. Authorization
models, workflow ladders, and platform caveats move to the installable skill
files, which already exist for this purpose and are already fetched on demand.
`browser_prepare`, `click`, `type_text`, and `get_window_state` are the four
worth doing first; together they carry over 25 KB.

**Hoist repeated envelope fields.** `risk.version` is emitted 54 times with
the same value while `capability_version` and `schema_version` already sit at
the top level of the response. Hoisting it is a `schema_version` bump for a
small, free saving, and is optional relative to the rest.

**Leave `outputSchema` alone.** The 15 identical copies of the 1,093-byte
action-result schema look like the most compressible 16 KB in the payload, and
they are not compressible within the MCP `tools/list` shape: there is no
cross-tool `$defs` pool a client is obliged to resolve. The schema was added
deliberately so that strict clients stop replacing actionable refusal messages
with opaque schema errors, which is itself one of the failure modes this RFC
exists to prevent. It stays.

The acceptance target is a full `tools/list` response under 60 KB with all 54
tools present, measured by the same capture that produced 126,461 bytes.

#### 3. Filter `tools/list` by the configured deny list

A tool named in `deny.tools`, or excluded by an `allow` list that names tools
explicitly, is omitted from `tools/list`.

This adds no knob. The operator has already declared the intent in a file the
product already reads; today that declaration is honored at invoke time and
ignored at list time. Honoring it in both places is the consistent behavior.

Two constraints:

- Enforcement does not move. Listing is presentational. A denied tool called
  anyway is refused at the native registry boundary with exactly the code and
  message it returns today, whether or not it appeared in the listing.
- Rule-constrained tools stay listed. A tool present in `allow.rules` with
  argument constraints is usable and must remain visible; only tools that can
  never succeed are hidden.

#### 4. Declare the `element_token` pattern

Add `"pattern": "^s[0-9a-f]{8}:[0-9]+$"` to the shared `element_token`
fragment, matching the format the driver has always minted and the format
already documented in `element_token.rs`.

This directly addresses an observed failure. A fabricated handle becomes a
client-side validation error at the point of the mistake, rather than a
round-trip that the model must then interpret. The change is additive: every
token the driver has ever minted matches.

#### 5. Lead `browser_prepare` success with the outcome

Restate the success text so the outcome and the next action come first, and
the non-effect assurance moves into the `side_effects` object that already
carries `launched_browser`, `created_profile`, and `reused_driver_profile`.

The information is preserved; only its position and its channel change.

#### 6. Compact defaults with the existing opt-ups

Converge `max_elements` and `max_depth` on one documented default per
parameter across macOS, Windows, and Linux. The current three-way drift —
including an uncapped Linux depth — is a defect independent of payload size.

Default `include_screenshot` on `get_window_state` to `false`.

This is the one Tier 1 item with real blast radius, and it is the clearest
statement of the compact-by-default principle. A caller that wants the
screenshot passes `include_screenshot: true`; the parameter already exists and
does not change. A caller that wanted only the element index stops paying for
an image it discards. `screenshot_out_file` continues to force a capture.

It is called out separately in the compatibility section rather than bundled
with the free wins, because it changes observable behavior for existing
callers.

### Tier 2: capability negotiation, kept narrow

Negotiation is for choices whose correct answer differs by client. Very few
qualify, and the bar should stay high, because each negotiated axis is a
behavior that must be tested in both states forever.

**MCP `initialize` is the surface.** It runs once per session, before any tool
call, and it already carries `clientInfo` and `capabilities`, which Cua Driver
already parses. Nothing in the model's context has to describe it, and nothing
in the model's reasoning has to account for it.

**A per-call argument is not the surface.** Adding `compact` or
`detail_level` to a tool pays the cost on every call: schema bytes in the
prefix, and a decision the model must make each time it acts. It also makes
the payload cost depend on the model correctly reasoning about payload cost,
which is exactly the capability that is scarce in the models this RFC is
trying to serve.

**Only one axis qualifies today.** Whether the client can accept images is a
real, declared, client-dependent fact — opencode's own configuration makes
input modalities explicit, and a build whose vision path is broken produces
confident wrong answers rather than errors. A client that declares no image
input should not be sent PNGs, and `include_screenshot: true` from such a
client should be answered with a structured explanation rather than an image.

Nothing else on the current surface has been shown to be model-dependent. The
rest of this RFC's savings are unconditional, which is why they are in Tier 1.

### On parameter proliferation

The count of new tool parameters, CLI flags, and environment variables in this
proposal is zero, and that is a design constraint rather than an accident.

A knob is what a proposal adds when it will not decide. It moves the cost from
the author to every caller, and in this specific domain the caller is a model
with a limited budget for decisions — the same budget the proposal claims to
be protecting. The measured payload makes the arithmetic concrete: a new
boolean property with a one-sentence description costs roughly 100 to 200
bytes on every tool that carries it, which is 5 to 10 KB of prefix if it
carries broadly, spent so that the default can remain worse than it should be.

The method is therefore: change the default, keep the opt-up that already
ships. `max_elements`, `max_depth`, `query`, `scope_ref`, `continuation`, and
`include_screenshot` already exist and stay exactly as they are.

## Alternatives considered

### Add a `compact` or `detail_level` argument

Rejected. It costs prefix bytes on every tool that carries it and a decision
on every call, in exchange for leaving the default unchanged. It also
presumes the model can reason about its own payload budget, which is the
capability least reliably present in the models this RFC serves.

### Ship a curated "minimal" tool profile behind a flag

Rejected in favor of Tier 1 item 3. Permission policy already lets an operator
declare which tools are usable, is already enforced, is already documented,
and is already loaded at startup. A second, parallel mechanism for expressing
the same intent would need its own precedence rules against the first.

### Drop `outputSchema` to recover 24,714 bytes

Rejected. It was added deliberately because strict MCP clients were replacing
refusal messages with `-32602` schema errors, which caused agents to abandon
the accessibility route for blind pixel clicking. Removing it would trade a
measured payload saving for a reintroduced correctness failure of exactly the
kind this RFC is trying to eliminate.

### Solve duplicate names with addressing rather than collapsing

`scope_ref` and `query` already exist, already work, and remain the right
tools for narrowing a large page. They do not solve this case: choosing
between four identically named refs requires knowing which one is correct
before issuing the query. Collapsing removes the choice instead of
re-presenting it.

### Fold roles globally rather than per cluster

Rejected as too blunt. `generic` is sometimes the only node carrying an
action, particularly on pages whose DOM supplement synthesized it. A global
role denylist would remove reachable elements. The cluster rule only collapses
where a same-named alternative provably survives.

### Negotiate a context budget at `initialize`

Deferred, not rejected — and deferred on evidence rather than on principle.

The idea is reasonable a priori: a client declares its window, and the driver
sizes snapshots to fit. But it is not supported by anything measured so far.
The two observed failures were fabricated handles and a misread success
message, both legibility problems. `semantic_v2` already pages at 300 nodes
with continuations, and `get_window_state` already caps elements, so a genuine
overflow would first surface as a non-zero `omitted.budget` or a truncated
element count. Neither appeared.

Building budget negotiation now would mean adding a permanently
dual-state behavior, and a second definition of "how much is too much"
alongside the constants that already exist, to fix a failure nobody has
observed.

The evidence that would justify it, stated so this can be revisited without
re-litigating:

- Trajectories in which `snapshot.omitted.budget > 0` or a truncated
  `element_count` correlates with task failure, after Tier 1 collapsing has
  landed and reduced the node count.
- A client whose context window cannot hold one already-compacted snapshot
  plus the reduced tool prefix — which, at a 60 KB prefix target, implies a
  window below roughly 32k tokens.
- Evidence that the fixed 300-node budget is wrong in both directions for
  different clients, rather than merely conservative for all of them.

If any of those appear, the negotiation belongs at `initialize` alongside the
image-capability axis, not as a per-call argument.

### Do nothing and let clients filter

Rejected. Some MCP clients can hide tools, but the filtering happens after the
server has already declared them, the mechanism differs per client, and it is
unavailable to SDK and CLI callers entirely. The redundancy is in the payload
Cua Driver produces, and that is where it should be fixed.

## Compatibility and migration

Most of the proposal is additive or content-only.

**Ref collapsing** changes snapshot content, not the ref namespace, the
continuation contract, or the response envelope. Refs are already opaque and
already scoped to a single snapshot that a navigation invalidates. `total_nodes`
and `selected_nodes` will report smaller numbers, which is the intended
observable effect. Collapsed roles are reported on the surviving node, so no
information about page structure is lost. A new omission counter records how
many nodes were collapsed, keeping the accounting complete.

**Description and shared-fragment canonicalization** is prose-only. No schema
shape changes, so no client that validates against the schema is affected.

**`tools/list` deny filtering** only removes tools an operator has already
made uncallable. A client that hard-codes a tool name it was already being
refused for will now see it absent instead; the refusal text on invocation is
unchanged, so the diagnostic path is preserved.

**The `element_token` pattern** matches every token the driver has ever
minted. A client that was sending valid tokens is unaffected. A client that
was sending invalid tokens was already failing, and now fails earlier and more
legibly.

**The `browser_prepare` text change** reorders prose in a text block. The
structured fields — `status`, `prepared`, `action`, `side_effects` — do not
change, and callers that read the structured content see no difference.

**Hoisting `risk.version`** changes the `tools/list` entry shape and requires a
`schema_version` bump. It is the lowest-value item in the proposal and can be
dropped without affecting the target.

### The `include_screenshot` default change

Defaulting `get_window_state`'s `include_screenshot` to `false` is a behavior
change for every existing caller that relied on the implicit screenshot. It
needs its own handling:

- Ship it in its own release, separated from the content reductions, with a
  release note that names the parameter and the one-line fix.
- Announce it one minor release ahead, with the current default retained and a
  note in the tool description.
- The rollback is a one-line default flip with no data migration and no
  contract change.

Review should settle whether this lands now or waits for image-capability
negotiation, so that the default follows what the client declared rather than
a fixed choice. That question is listed as unresolved.

### Rollback

Every Tier 1 item is independently revertible. Ref collapsing reverts to the
parent-only rule. Description trimming reverts by restoring text. Deny
filtering reverts to the unfiltered listing with enforcement untouched
throughout. The `element_token` pattern reverts by deleting one schema key.
None of them require a data migration or leave persistent state behind.

## Security, privacy, and telemetry

**No permission boundary moves.** This must be stated plainly because item 3
touches a security-adjacent surface: filtering `tools/list` by policy is
presentational only. Enforcement stays at the native registry boundary, where
SDK, CLI, MCP, and raw-socket calls all converge, and a denied tool called by
name is refused identically whether or not it was listed. A reviewer should
treat any implementation that makes listing load-bearing for authorization as
incorrect.

**Policy contents are not disclosed by listing.** Absence of a tool from
`tools/list` reveals that the tool is unavailable, which is the intended
signal. It must not reveal the policy's rules, its constraints, its file path,
or its hash beyond what `cua-driver status` already reports.

**Defaulting `include_screenshot` to `false` reduces incidental capture.**
Fewer screenshots reach client transcripts and logs by default, and a caller
that needs one asks for it explicitly. This is a small privacy improvement
that falls out of the change rather than motivating it.

**Negotiation must not start retaining client data.** The `initialize`
metadata path today truncates each value to 128 characters and retains
nothing. An image-capability axis reads one presence flag at handshake time
and holds it for the connection's lifetime. It must not begin retaining or
forwarding `clientInfo`, and it adds no telemetry.

**Collapsing must not conceal an interactive element.** The security-relevant
failure mode for item 1 is a snapshot that omits something the user can click,
because that makes the driver's report of the page incomplete. The bounding
rules and the acceptance tests exist for this reason, and the collapsed-node
count is reported so the omission accounting stays complete.

## Implementation plan

Each slice is independently reviewable, independently revertible, and gated on
its own evidence.

### Slice A: Establish the payload baseline

Add a test that captures the full `tools/list` response, asserts a byte
ceiling, and prints the per-component breakdown on failure. Land it against
today's 126,461 bytes so every later slice's saving is measured rather than
asserted.

### Slice B: Canonicalize shared schema fragments

Extend the drift gate's reducer to compare `description` and `pattern`. Reduce
each shared fragment to one description. Add the `element_token` pattern in
the same slice, since it is the same mechanism. Gate: the drift test fails on
a reintroduced divergence, and a fabricated token fails client-side
validation.

### Slice C: Trim tool descriptions

Move authorization models, workflow ladders, and platform caveats from tool
descriptions into the installable skill files, starting with
`browser_prepare`, `click`, `type_text`, and `get_window_state`. Gate:
the Slice A ceiling drops to the 60 KB target, and the skill files cover
everything removed.

### Slice D: Collapse ref clusters

Implement the cluster rule and its bounding constraints, extend the omission
accounting with a collapsed count, and report collapsed roles on the surviving
node. Gate: the fixture tests below, and a re-measured node count on a
results-page fixture.

### Slice E: Filter `tools/list` by policy

Apply the deny list and explicit allow list to the listing. Leave every
enforcement path untouched. Gate: a denied tool is absent from the listing and
still refused identically on invocation.

### Slice F: Repair `browser_prepare` success reporting

Reorder the success text and move the non-effect assurance into
`side_effects`. Gate: structured fields byte-identical to today; text leads
with the outcome.

### Slice G: Converge snapshot defaults

Converge `max_elements` and `max_depth` across platforms. Ship the
`include_screenshot` default flip separately, after the announcement window,
and only if review resolves the unresolved question in its favor.

### Later: Image-capability negotiation

Read the client's declared image capability at `initialize`, hold it for the
connection, and shape screenshot-bearing responses accordingly. Deferred until
Tier 1 has landed and its savings are measured, so the negotiation is sized
against the reduced payload rather than the current one.

## Test and acceptance plan

The RFC is implemented when all of the following evidence passes.

### Payload

- A captured `tools/list` response is under the stated ceiling with all 54
  macOS tools present, and the equivalent Windows and Linux rosters are
  captured and bounded too.
- The drift gate fails when a shared fragment ships a divergent description or
  loses its `pattern`.
- Every tool retains its `inputSchema`, `annotations`, `capabilities`, `risk`,
  and, where it has one, its `outputSchema`.
- The installable skill files cover every workflow, authorization, and
  platform caveat removed from a tool description.

### Snapshot collapsing

- A fixture with four identically named nodes per result — `heading`, `link`,
  `generic`, `statictext` — emits one actionable ref per result.
- A fixture where the only actionable node shares its name with a wrapper
  still emits that actionable node.
- A fixture where two same-named nodes both carry actions resolves by the
  documented rule and emits the documented survivor.
- Collapsing never crosses a frame boundary or an actionable ancestor.
- Content refs that name text no actionable ref names are retained.
- The collapsed count appears in the omission accounting, and
  `total_nodes` plus omissions still account for every candidate.
- The existing parent-only static-text behavior remains covered.

### Listing and enforcement

- A tool in `deny.tools` is absent from `tools/list`.
- The same tool called by name is refused with the same code and message as
  today.
- A tool present in `allow.rules` with argument constraints remains listed.
- With no policy configured, the listing is unchanged apart from the payload
  reductions.
- SDK, CLI, MCP, and raw-socket paths all show unchanged enforcement.

### Handles and refusals

- A fabricated `element_token` fails schema validation at a strict client.
- A valid `element_token` still resolves, and a stale one still returns
  `stale_element_token` with its current message.
- `browser_prepare` success text states the outcome before any non-effect
  prose; `status`, `prepared`, `action`, and `side_effects` are unchanged.

### Cross-platform and end-to-end

- `max_elements` and `max_depth` report the same defaults on macOS, Windows,
  and Linux, and Linux is no longer uncapped.
- With `include_screenshot` unset, `get_window_state` returns no image and the
  element index is unchanged; with it set to `true`, behavior is as today;
  `screenshot_out_file` still forces a capture.
- The local-model trajectory that produced this RFC's evidence is re-run and
  reports first-inference time, per-step time, and the node count for the same
  results page.

## Unresolved questions

- Should `include_screenshot` default to `false` now, or wait for
  image-capability negotiation so the default follows the client's declaration
  rather than a fixed choice?
- When several nodes in a cluster carry actions, which survives — the most
  specific role, or the largest action set?
- Should deny-list filtering of `tools/list` be unconditional, or should an
  operator be able to keep denied tools listed so that refusals remain legible
  to the model?
- What is the right byte ceiling? 60 KB is derived from halving the measured
  payload; a target derived from a real client's budget would be better.
- Is the desktop accessibility snapshot redundant in the same way the browser
  snapshot is? Nothing should be proposed for it before it is measured the way
  the browser side was.
- Should the drift gate's description comparison be exact, or bounded by a
  length budget that permits per-tool specificity?

## Decision record

Pending review.
