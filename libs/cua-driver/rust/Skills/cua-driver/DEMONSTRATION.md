# Demonstrations → skills

> A demonstration-to-skill workflow. Record a human performing a task on a
> window, then author a reusable `SKILL.md` from the resulting trajectory.
> Generalization happens
> in natural language — the skill describes *intent*, not pixel coordinates, so
> it replays against a live screen rather than a brittle coordinate script.
>
> Recording how demonstration mode works + its security model lives in
> `RECORDING.md`. This file is about turning a finished recording into a skill.

## 1. Finish the demonstration

Call `stop_demonstration`. It writes, next to the `turn-*/` folders:

- **`TRAJECTORY.md`** — readable action prose; screenshots referenced by
  **relative path**, only a few key frames embedded inline. Read THIS, not the
  raw `action.json` tree — it will not blow up your context with base64.
- **`SUMMARY.json`** — action count, duration, completeness, dropped-event and
  screenshot-failure counts, and the action histogram.
- **`DEMONSTRATION.json`** — schema and driver versions, target identity,
  timestamps, stop reason, and capture completeness.

## 2. Read the trajectory and identify the task

Read `TRAJECTORY.md`. Open at most one or two of the linked
`turn-*/screenshot.png` / `click.png` frames — only when an action's intent is
unclear from the prose. Determine:

- **What goal** the demonstration accomplishes (a sentence).
- **The inputs** that varied or would vary across runs (names, dates, file
  paths, search terms). These become `{placeholders}`.
- **The meaningful steps** — collapse incidental turns (stray clicks, scrolls to
  bring something into view, redacted typing that was just navigation) into
  intent. Several low-level turns often map to one semantic step.

## 3. Author SKILL.md (Open Agent Skills Standard)

Write a single `SKILL.md` in the Open Agent Skills Standard:

```markdown
---
name: kebab-case-name
description: Use when <trigger>. Do NOT use when <anti-trigger>.
---

# <name>

<one-line overview of what the task accomplishes>

## Inputs
- `{placeholder_one}` — what it is
- `{placeholder_two}` — what it is

## Steps
1. Open <app/URL>
2. Click the **Create New Report** button
3. Enter the title as `{month}-{team}-expenses`
4. <...>

## Verification
- <how to confirm success — what on screen proves the task completed>
- <brief recovery note if a step doesn't land>
```

### Rules (what makes a skill good, not just correct)

- **Frontmatter is two keys only**: `name` and `description`. The `description`
  is a routing signal — front-load "Use when…"/"Do NOT use when…", not a
  summary. All "when to use" info goes here, not in the body.
- **Steps are semantic, never coordinates.** Write "Click the **Save** button",
  not "click at (412, 880)". Recorded pixel coordinates are in the trajectory
  for reference, but a coordinate-replay skill breaks on any layout change.
  Refer to elements by visible name/role.
- **Generalize literal text into `{placeholders}`.** Default capture records
  only `text entered (redacted)` — never invent the literal value; represent it
  as an input under `## Inputs`.
- **Never invent or hardcode secrets.** Text input events are always redacted,
  but screenshots can contain visible content. Review them before sharing and
  make varying or sensitive values explicit inputs.
- **Keep it minimal.** One screen of steps beats a transcript. Drop turns that
  were just navigation.
- **Always add a Verification section.** A skill the agent can't self-check is a
  skill that fails silently.

## 4. Sanity check

- Could a fresh agent follow these steps on a slightly different screen? If a
  step only makes sense at one resolution/position, re-write it semantically.
- Are all varying values `{placeholders}` listed under `## Inputs`?
- Does `description` make clear when NOT to trigger?

## Where skills go

cua-driver itself does not pick a skills directory for you — write the
`SKILL.md` wherever your harness loads skills from (e.g. Claude Code
`~/.claude/skills/<name>/`). The recording's
`output_dir` is a fine staging spot; copy the finished skill into your harness's
skills directory.
