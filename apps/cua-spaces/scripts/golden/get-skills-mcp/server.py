#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""get-skills — a minimal MCP server that hands an agent a skill per app.

Skills-over-MCP is not yet a first-class feature in every agent, so this is the
fallback: one markdown file per application describing that app's *official*
agent integration (which MCP to use, its exact tool names, the workflows that
actually work in this image, and the traps that do not).

Two tools:
  list_skills          -> the available skill names and their one-line summaries
  get_skill(name)      -> the full markdown for one skill

Deliberately dependency-free (stdlib only, speaking MCP over stdio), so it runs
on the system python3 without a virtualenv and cannot break because of a
resolver change.
"""
import json
import os
import sys

SKILLS_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "skills")
PROTOCOL_VERSION = "2024-11-05"


def skill_files():
    """Map skill name -> path, e.g. {"unity": ".../skills/unity.md"}."""
    if not os.path.isdir(SKILLS_DIR):
        return {}
    return {
        os.path.splitext(f)[0]: os.path.join(SKILLS_DIR, f)
        for f in sorted(os.listdir(SKILLS_DIR))
        if f.endswith(".md")
    }


def summary_of(path):
    """First non-empty, non-heading line — used as the skill's description."""
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                line = line.strip()
                if line and not line.startswith("#"):
                    return line[:200]
    except OSError:
        pass
    return ""


def tool_list():
    names = ", ".join(skill_files()) or "(none installed)"
    return [
        {
            "name": "list_skills",
            "description": (
                "List the app skills available here, and READ THE RELEVANT ONES "
                "BEFORE PLANNING. They are not optional reference material: they "
                "define what the user MEANS when they ask for app work in a Cua "
                "Space, which differs from how app automation usually works, and "
                "they record what actually succeeds in this image — including "
                "approaches that look reasonable and silently fail.\n"
                "\n"
                "In particular, the user's project/scene/file normally lives in "
                "their CLOUD account, reachable with credentials already present on "
                "this machine. Its absence from local disk is the expected starting "
                "state and usually the FIRST STEP of the task, not a blocker. If "
                "anything the request needs looks missing — a project, a checkout, "
                "an asset — read that app's skill before reporting it as impossible; "
                "the skill documents how to fetch it.\n"
                "Applications in a Space are HOT-LOADED: an app's MCP tools are "
                "listed and callable whether or not the application is on screen, and "
                "the first call starts it. A closed app is not a missing capability — "
                "the skills below are the list of what this Space can do.\n"
                f"Available: {names}."
            ),
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False},
        },
        {
            "name": "get_skill",
            "description": (
                "Return the full markdown for one skill. Read the relevant skill "
                "BEFORE driving an app — it names the exact tools, the verified "
                "workflows, and the approaches that look reasonable but do not work. "
                "If something the task needs appears to be MISSING from this machine "
                "(a project, an asset, a checkout), read that app's skill before "
                "reporting it as a blocker: the skill usually documents how to fetch it."
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": f"Skill name, one of: {names}",
                    }
                },
                "required": ["name"],
                "additionalProperties": False,
            },
        },
    ]


def call_tool(name, args):
    files = skill_files()
    if name == "list_skills":
        if not files:
            return f"No skills installed (looked in {SKILLS_DIR})."
        return "\n".join(f"- {n}: {summary_of(p)}" for n, p in files.items())
    if name == "get_skill":
        wanted = (args or {}).get("name", "")
        # Tolerate "unity.md" and case differences rather than failing outright.
        key = os.path.splitext(str(wanted))[0].strip().lower()
        for n, p in files.items():
            if n.lower() == key:
                with open(p, encoding="utf-8", errors="replace") as fh:
                    return fh.read()
        return f"No such skill {wanted!r}. Available: {', '.join(files) or '(none)'}"
    raise ValueError(f"unknown tool {name!r}")


def respond(msg_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": msg_id, "result": result}) + "\n")
    sys.stdout.flush()


def respond_error(msg_id, code, message):
    sys.stdout.write(
        json.dumps({"jsonrpc": "2.0", "id": msg_id, "error": {"code": code, "message": message}})
        + "\n"
    )
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            continue

        method = msg.get("method")
        msg_id = msg.get("id")

        # Notifications carry no id and must never be answered.
        if msg_id is None:
            continue

        if method == "initialize":
            respond(msg_id, {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "get-skills", "version": "1.0.0"},
                "instructions": (
                    "Per-app skills for this Space. Call list_skills, then get_skill "
                    "for the app you are about to drive."
                ),
            })
        elif method == "tools/list":
            respond(msg_id, {"tools": tool_list()})
        elif method == "tools/call":
            params = msg.get("params") or {}
            try:
                text = call_tool(params.get("name"), params.get("arguments"))
                respond(msg_id, {"content": [{"type": "text", "text": text}]})
            except Exception as exc:  # surface as a tool error, not a crash
                respond(msg_id, {
                    "content": [{"type": "text", "text": f"error: {exc}"}],
                    "isError": True,
                })
        else:
            respond_error(msg_id, -32601, f"method not found: {method}")


if __name__ == "__main__":
    main()
