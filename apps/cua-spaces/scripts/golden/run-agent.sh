#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# run-agent.sh — start the in-Space coding agent on the demo prompt.
#
# MUST run in the GUI (Aqua) session. This script is normally started over SSH,
# and an SSH login lands in a launchd **Background** session. In that session
# every Keychain read fails with errSecInteractionNotAllowed (rc=36) — silently,
# as an ordinary "not signed in" result rather than an error. `unity doctor`
# there reports `credential-store fail` and `auth.loggedIn false`, while the
# very same command in the Aqua session reports `pass` / `true`.
#
# So an agent spawned straight from SSH inherits the Background session and
# every keychain-backed Unity operation fails invisibly, which looks exactly
# like a dead login. Re-exec into the Aqua session with `launchctl asuser`
# before doing anything else.
export PATH="/Users/lume/.local/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

UID_NUM="$(id -u)"
# `launchctl asuser <uid> ...` puts the child in the user's GUI session. Guard
# with CUA_AGENT_IN_AQUA so the re-exec happens exactly once.
if [ -z "${CUA_AGENT_IN_AQUA:-}" ] && [ "$(launchctl managername 2>/dev/null)" != "Aqua" ]; then
  export CUA_AGENT_IN_AQUA=1
  exec /bin/launchctl asuser "$UID_NUM" /bin/bash "$0" "$@"
fi

RUN="${1:-run}"
PROMPT="${2:-teleport my unity hub to a new lume cua-space, open biome-tiles scene & blender, add a hoverboard and spawn it next to the player in Assets/Scenes/Overworld.unity}"
/Users/lume/.cua/write-agent-mcp.sh >/dev/null 2>&1
WORK="/Users/lume/.spaces-agents/${RUN}/work"; mkdir -p "$WORK"; cd "$WORK"
# The user's prompt is passed VERBATIM. Everything the agent needs to know about
# how it is being run goes in the system prompt instead, so the demo prompt stays
# exactly what a user would type.
#
# `claude -p` is SINGLE-SHOT: when the turn ends the process exits and there is
# no later turn to resume into. An agent that does not know this reasons like an
# interactive session and defers -- one run ended with "I'll pause here and pick
# up as soon as the import finishes", exiting 0 with the task untouched, having
# already modelled the asset. Long waits are normal here (a first Unity import
# is minutes), so the agent must wait IN-PROCESS rather than hand work back.
RUN_CONTEXT='You are running headless and SINGLE-SHOT: when this turn ends your
process exits and nothing resumes it. There is no later, no follow-up turn, and
no one to hand work back to. Never end a turn with work outstanding or promise
to continue afterwards.

Long waits are expected and are not a reason to stop. A first Unity import takes
several minutes; sleep and poll in-process until it finishes, then carry on. If
something looks stuck for more than about 90 seconds, look at the screen before
assuming progress -- a blocked app is usually waiting on a dialog, not working.

Finish the whole task in this one turn, then report what you did.'

exec claude -p "$PROMPT" --append-system-prompt "$RUN_CONTEXT" \
  --mcp-config /Users/lume/.cua/agent-mcp.json --strict-mcp-config \
  --dangerously-skip-permissions --output-format stream-json --verbose \
  > "/Users/lume/.cua/${RUN}.jsonl" 2> "/Users/lume/.cua/${RUN}.err" < /dev/null
