#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Cua AI, Inc.
"""How much of a recorded agent run used run_steps / run_script, and how much could have.

Input: a `calls.parquet` with one row per tool call (columns os, run, trial_id, i, tool,
input), such as the public Scale AgentEnv dataset for Cua Driver Bench.

    python3 measure-batching.py calls.parquet [os=macos] [run-substring=cua-driver]

A "group" is a run of single action calls (click, type, key, set value, scroll, drag,
menu) with an optional get_window_state right after it. A group of two or more calls
could have been one run_steps call with observe:{}.
"""
import json
import re
import sys

import pandas as pd

ACTIONS = {"click", "double_click", "right_click", "type_text", "press_key", "hotkey",
           "set_value", "scroll", "drag", "invoke_menu"}
BATCH = {"run_steps", "run_script"}
SCRIPT_CALL = re.compile(
    r"\b(?:app|cua)\.(?:click|doubleClick|rightClick|typeText|type|pressKey|press|hotkey"
    r"|setValue|scroll|drag|invokeMenu|menu)\(")


def steps_in(tool, raw):
    try:
        data = json.loads(raw)
    except Exception:
        return 1
    if tool == "run_steps":
        return max(1, len(data.get("steps", [])))
    return max(1, len(SCRIPT_CALL.findall(data.get("script", ""))))


def trial_stats(tools, inputs):
    n = len(tools)
    batch_calls = [i for i, t in enumerate(tools) if t in BATCH]
    stats = dict(calls=n, batch=len(batch_calls),
                 single=sum(t in ACTIONS for t in tools),
                 steps=sum(steps_in(tools[i], inputs[i]) for i in batch_calls),
                 reads_after_single=0, savable=0, groups=0)
    j = 0
    while j < n:
        if tools[j] not in ACTIONS:
            j += 1
            continue
        start = j
        while j < n and tools[j] in ACTIONS:
            j += 1
        size = j - start
        if j < n and tools[j] == "get_window_state":
            stats["reads_after_single"] += 1
            size += 1
            j += 1
        if size >= 2:
            stats["groups"] += 1
            stats["savable"] += size - 1
    return stats


def main(path, os_name="macos", run_filter="cua-driver"):
    calls = pd.read_parquet(path)
    calls = calls[(calls.os == os_name) & calls.run.str.contains(run_filter)].copy()
    calls["tool"] = calls.tool.str.split("__").str[-1]
    rows = []
    for (run, _), group in calls.groupby(["run", "trial_id"]):
        group = group.sort_values("i")
        rows.append(dict(run=run.split("-2026")[0],
                         **trial_stats(list(group.tool), list(group.input))))
    frame = pd.DataFrame(rows)
    for name, part in [*frame.groupby("run"), ("ALL", frame)]:
        calls_n, batch = part.calls.sum(), part.batch.sum()
        actions = part.single.sum() + part.steps.sum()
        print(f"{name}: trials={len(part)} calls={calls_n}")
        print(f"  batch calls: {batch} ({100 * batch / calls_n:.1f}% of calls); "
              f"trials using a batch: {100 * (part.batch > 0).mean():.0f}%")
        print(f"  actions inside a batch: {100 * part.steps.sum() / actions:.0f}%; "
              f"actions per batch call: {part.steps.sum() / max(batch, 1):.2f}")
        print(f"  single-action groups of 2+ calls: {part.groups.sum()}; calls one batch "
              f"would save: {part.savable.sum()} ({100 * part.savable.sum() / calls_n:.1f}% of calls)")
        print(f"  get_window_state right after a single action: {part.reads_after_single.sum()}")


if __name__ == "__main__":
    main(*sys.argv[1:4])
