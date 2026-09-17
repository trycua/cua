---
name: jev-use
description: Build or adapt a bounded computer-use loop where Cua Driver observes and acts, TypeSafe Jev selects only from application-owned candidate IDs, and the caller validates and verifies every action. Use for the jev-use recipe or similar Jev integrations; do not use it to add model logic or credentials to Cua Driver.
---

# jev-use

Keep the decision layer above Cua Driver. Driver supplies observations and
executes actions; the application constructs complete candidates; TypeSafe Jev
returns one candidate ID. Never let Jev invent tool names, coordinates, refs,
targets, delivery modes, or other arguments.

Use the example at `libs/cua-driver/examples/jev-use/` as the runnable reference.
Its current live path uses browser DOM and semantic evidence. Visual perception
is a future optional adapter whose schema and tool contract are not defined by
this skill.

## Decision loop

1. State the goal and obtain a fresh Cua Driver observation through one
   persistent CLI or MCP session.
2. Prefer an unambiguous fresh accessibility or browser DOM token.
3. If visual grounding is needed, use an accepted and available generic
   perception contract when one exists, retaining the current capture metadata.
   Otherwise reobserve or abstain rather than inventing visual evidence.
4. Construct a bounded candidate table. Each executable candidate contains the
   complete Driver tool and arguments. Include `reobserve` and `abstain` when
   evidence can be stale, incomplete, or ambiguous.
5. Send Jev only the goal, compact observation, recent history, and candidate
   IDs with descriptions.
6. Resolve the returned ID against the original table. Reject an unknown ID,
   stale observation, disallowed action, or result below the caller's stated
   confidence policy.
7. Execute at most one Driver action. Use background delivery by default;
   foreground delivery is an explicit escalation subject to the active Driver
   contract and user authorization.
8. Reobserve and verify the postcondition before building another table.

## Freshness and visual evidence

- Treat Driver page refs, accessibility tokens, screenshot IDs, and visual
  region IDs as observation-local. Never reuse them after the UI changes.
- Require visual bounds and centers to remain inside the exact screenshot
  coordinate space and tied to the same target and snapshot.
- If semantic and visual evidence disagree, or multiple regions are plausible,
  offer `reobserve` and `abstain` without inventing a mutation.
- Do not claim that an unreleased perception tool, schema, or adapter exists.

## Credentials and proof

The deterministic mock path must work without `TYPESAFE_API_KEY`. For live Jev,
read the key from the process environment or a secure interactive prompt; never
put it in source, command arguments, logs, artifacts, or messages. Verify task
completion from an independent application postcondition rather than a model
answer, action response, or screenshot alone.
