# Demonstrations → skills

> A record-and-replay demonstration-to-skill workflow. Record a human (or
> agent) demonstrating a task on a window, process it into a readable
> trajectory, then author a reusable `SKILL.md` from it. Generalization happens
> in natural language — the skill describes *intent*, not pixel coordinates, so
> it replays against a live screen rather than a brittle coordinate script.
>
> Recording how demonstration mode works + its security model lives in
> `RECORDING.md`. This file is about turning a finished recording into a skill.

## 1. Process the recording

Call `process_recording({ "dir": "<output_dir>" })` (the `output_dir` you passed
to `start_recording`). It writes, next to the `turn-*/` folders:

- **`TRAJECTORY.md`** — readable action prose; screenshots referenced by
  **relative path**, only a few key frames embedded inline. Read THIS, not the
  raw `action.json` tree — it will not blow up your context with base64.
- **`SUMMARY.json`** — step count, duration, human/agent split, action
  histogram.

If `ANTHROPIC_API_KEY` is set and you pass `author_skill: true`,
`process_recording` calls the Anthropic API to draft a `SKILL.md` for you —
review and refine it with the rules below rather than shipping it blind.

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
- **Generalize literal text into `{placeholders}`.** Typed text in the
  trajectory is redacted by default (e.g. `•••• (8 chars, alpha)`) — never
  invent the literal value; represent it as an input under `## Inputs`.
- **Never hardcode secrets.** Anything captured in raw mode (`capture_raw_text`)
  that is a password/token must become an input, not a literal.
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
