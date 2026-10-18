#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# install-skills.sh — install the get-skills MCP and its per-app skills.
#
# Skills-over-MCP is not yet first-class in every agent, so this is the
# fallback: a tiny stdio MCP server exposing one markdown skill per app, each
# describing that app's OFFICIAL agent integration — the exact tool names, the
# workflows verified in this image, and the approaches that look reasonable but
# silently fail (pixel-clicking Unity Hub's WebKit dropdowns, for one).
#
# Run inside the guest with this script's directory available, e.g.
#   scp -r scripts/golden lume@<ip>:~/golden-src && ssh … 'bash ~/golden-src/install-skills.sh'
set -uo pipefail

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="$HOME/.cua/demo-kit/get-skills-mcp"

say() { printf '\033[1;36m==> %s\033[0m\n' "$*"; }

[ -f "$SRC/get-skills-mcp/server.py" ] || {
  echo "server.py not found next to this script (looked in $SRC/get-skills-mcp)" >&2
  exit 1
}

say "Installing get-skills MCP -> $DEST"
mkdir -p "$DEST/skills"
install -m 755 "$SRC/get-skills-mcp/server.py" "$DEST/server.py"

# Prefer the canonical skills/ dir; fall back to the copy beside the server.
if compgen -G "$SRC/skills/*.md" >/dev/null; then
  install -m 644 "$SRC"/skills/*.md "$DEST/skills/"
elif compgen -G "$SRC/get-skills-mcp/skills/*.md" >/dev/null; then
  install -m 644 "$SRC"/get-skills-mcp/skills/*.md "$DEST/skills/"
else
  echo "no skill markdown found" >&2; exit 1
fi

say "Installed skills"
for f in "$DEST"/skills/*.md; do
  printf '  %-12s %s lines\n' "$(basename "$f")" "$(wc -l < "$f" | tr -d ' ')"
done

# --------------------------------------------------------------------------
# Belt and braces: the same skills as REAL agent skills in ~/.claude/skills.
# --------------------------------------------------------------------------
# The get-skills MCP above only helps an agent that was launched with
# `--mcp-config ~/.cua/agent-mcp.json`. In the last acceptance run the in-Space
# agent was launched without it, so it could not reach the MCP at all: it had no
# Blender skill whatsoever, and had to bootstrap a Unity one for itself with
# `unity skill install claude-code --local`. ~/.claude/skills did not exist.
#
# A skill on disk needs no MCP config, no server process and no wiring — the
# agent picks it up from $HOME. So install both, and stop the reachability of
# the skills depending on how the agent happens to be started.
#
# Layout is the agent's own: ~/.claude/skills/<name>/SKILL.md, with YAML
# frontmatter carrying `name` and `description`. The source markdown has no
# frontmatter (the MCP serves it raw), so it is generated here from the first
# real paragraph — the same text summary_of() in server.py uses — collapsed to
# one line, because a folded multi-line description is not worth the quoting
# risk. The body is copied through byte for byte, so the two surfaces serve
# identical text.
CLAUDE_SKILLS="$HOME/.claude/skills"
say "Installing agent skills -> $CLAUDE_SKILLS"
mkdir -p "$CLAUDE_SKILLS"
/usr/bin/python3 - "$DEST/skills" "$CLAUDE_SKILLS" <<'PY'
import os, re, sys

src, dest = sys.argv[1], sys.argv[2]
for fn in sorted(os.listdir(src)):
    if not fn.endswith(".md"):
        continue
    name = os.path.splitext(fn)[0]
    body = open(os.path.join(src, fn), encoding="utf-8").read()

    # First paragraph that is not a heading, as one line.
    desc = ""
    for para in re.split(r"\n\s*\n", body):
        para = para.strip()
        if para and not para.startswith("#"):
            desc = " ".join(para.split())
            break
    # Strip markdown emphasis and quote-escape for a plain YAML scalar.
    desc = re.sub(r"[*_`]", "", desc)[:400].replace('"', "'")
    desc = f"{desc} Read this before planning any {name} work in a Cua Space."

    out = os.path.join(dest, name)
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "SKILL.md"), "w", encoding="utf-8") as fh:
        fh.write(f'---\nname: {name}\ndescription: "{desc}"\n---\n\n')
        fh.write(body)
    print(f"  {name}/SKILL.md")
PY

# Prove an agent would actually see them, rather than trusting the write.
say "Verifying installed skills"
for d in "$CLAUDE_SKILLS"/*/; do
  f="$d/SKILL.md"
  [ -f "$f" ] || { echo "MISSING: $f" >&2; exit 1; }
  head -1 "$f" | grep -q '^---$' || { echo "no frontmatter in $f" >&2; exit 1; }
  grep -q '^name: ' "$f" || { echo "no name: in $f" >&2; exit 1; }
  grep -q '^description: ' "$f" || { echo "no description: in $f" >&2; exit 1; }
  printf '  ok  %-28s %s lines\n' "${d#$CLAUDE_SKILLS/}SKILL.md" "$(wc -l < "$f" | tr -d ' ')"
done

say "Smoke-testing the server (stdio MCP handshake)"
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_skills","arguments":{}}}' \
  | /usr/bin/python3 "$DEST/server.py" \
  | /usr/bin/python3 -c '
import json,sys
ok=0
for line in sys.stdin:
    d=json.loads(line); r=d.get("result",{})
    if "serverInfo" in r: print("  init:", r["serverInfo"]["name"]); ok+=1
    elif "tools" in r:    print("  tools:", [t["name"] for t in r["tools"]]); ok+=1
    else:
        print("  skills:", r.get("content",[{}])[0].get("text","")[:70].replace("\n"," | ")); ok+=1
sys.exit(0 if ok==3 else 1)'

say "get-skills MCP ready"
echo "Register it with an agent as:"
echo "  \"get-skills\": {\"command\": \"/usr/bin/python3\", \"args\": [\"$DEST/server.py\"]}"
