#!/bin/sh
# A fake Claude Code CLI for the openkoalabots scenario.
#
# It never calls a model, opens no network connection and reads no
# credentials. The scenario installs it in the Space as ~/.local/bin/claude
# (the agent harness puts ~/.local/bin first on PATH), so `agent_start`
# (`claude -p <prompt> ...`) and `agent_message` (`claude -p <msg> --continue`)
# drive the real harness plumbing: process tags, output tails, run status.
prompt=""
cont=0
while [ $# -gt 0 ]; do
  case "$1" in
    -p|--print) prompt="$2"; shift 2 ;;
    --continue|-c) cont=1; shift ;;
    *) shift ;;
  esac
done
# The harness prepends an operating-mode preamble to the first prompt, and an
# app may add a bracketed metadata tag (e.g. "[openkoalabots:inbox]") on its own
# line or in front of the words; the
# user's words are the last other non-empty line. A real agent never repeats
# the tag, so neither does this one.
prompt="$(printf '%s\n' "$prompt" | awk 'NF && $0 !~ /^[[:space:]]*\[.*\][[:space:]]*$/ { last = $0 } END { print last }')"
prompt="$(printf '%s' "$prompt" | sed 's/^[[:space:]]*\[[^]]*\][[:space:]]*//')"
# Prompts the Swift live suites send: "... `sleep 45` ..." keeps the turn in
# flight (for the mid-turn refusal tests); "the single word X" also prints X
# on its own line, as a real agent's answer would.
case "$prompt" in
  *'`sleep '[0-9]*)
    n="$(printf '%s' "$prompt" | sed -n 's/.*`sleep \([0-9][0-9]*\)`.*/\1/p')"
    echo "fake claude: working"
    sleep "${n:-5}"
    ;;
esac
if [ "$cont" = 1 ]; then
  echo "fake claude continued: $prompt"
else
  echo "fake claude did: $prompt"
fi
word="$(printf '%s' "$prompt" | sed -n 's/.*single word \([A-Za-z0-9_-]*\).*/\1/p')"
[ -n "$word" ] && echo "$word"
exit 0
