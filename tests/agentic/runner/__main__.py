"""bin/agentic-e2e: prompt tests.

  bin/agentic-e2e validate [SPEC...]        check every spec (default: all)
  bin/agentic-e2e list [--os OS]            the tests, and which have a trajectory
  bin/agentic-e2e select --changed FILE     tests whose area a list of changed paths touches
  bin/agentic-e2e run [SPEC...] [options]   provision, run, check, report

run options:
  --mode live|replay|auto   auto (default): replay a saved trajectory, live when
                            there is none or it breaks; replay: never call the model
  --target local|ssh://user@host?os=linux
  --target-env K=V          environment for the target's desktop session (repeat)
  --ephemeral               this machine is a throwaway runner (CI): default homes
                            and system installs are allowed
  --build release:<tag>|run:<id>|file:<path>   (default in config.yaml)
  --driver-bin PATH         cua-driver on the target (default: cua-driver)
  --record                  save the trajectory of a passing live run
  --review                  add an advisory model review to the report
  --out DIR                 reports and artifacts (default: artifacts/agentic-e2e)

Needs CUA_E2E_CANARY_USERNAME / CUA_E2E_CANARY_PASSWORD (the staging canary
test account) and, for live runs, ANTHROPIC_API_KEY or a signed-in `claude`.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import secrets
import subprocess
import sys
import tempfile
import time
import traceback
from pathlib import Path

import yaml

from . import agent, canary, checks, provision, report
from . import spec as specmod
from . import trajectory

HERE = Path(__file__).resolve().parents[1]
REPO = HERE.parents[1]


def load_config() -> dict:
    cfg = yaml.safe_load((HERE / "config.yaml").read_text(encoding="utf-8"))
    if cfg.get("environment") != "staging":
        raise SystemExit("config.yaml: prompt tests run against staging only")
    return cfg


def resolve(paths: list[str]) -> list[Path]:
    if not paths:
        return specmod.discover()
    out = []
    for p in paths:
        q = Path(p)
        if not q.exists():
            q = HERE / (p if p.endswith(".yaml") else p + ".yaml")
        out.append(q)
    return out


def cmd_validate(args) -> int:
    bad = 0
    for p in resolve(args.specs):
        try:
            s = specmod.load(p)
            print(f"ok   {s.key} ({', '.join(s.os)}; {len(s.expect)} checks)")
        except specmod.SpecError as e:
            bad += 1
            print(f"FAIL {e}")
    return 1 if bad else 0


def cmd_list(args) -> int:
    for p in resolve([]):
        s = specmod.load(p)
        if args.os and args.os not in s.os:
            continue
        have = [
            o
            for o in s.os
            if (trajectory.trajectory_dir(s.area, s.name, o) / "portable.json").exists()
        ]
        print(f"{s.key}\tos={','.join(s.os)}\ttrajectories={','.join(have) or '-'}")
    return 0


def cmd_select(args) -> int:
    cfg = load_config()
    changed = [ln.strip() for ln in Path(args.changed).read_text().splitlines() if ln.strip()]
    areas = {
        a
        for a, globs in (cfg.get("areas") or {}).items()
        if any(fnmatch.fnmatch(f, g) for f in changed for g in globs)
    }
    for p in resolve([]):
        s = specmod.load(p)
        if s.area in areas and (not args.os or args.os in s.os):
            print(s.path.relative_to(REPO))
    return 0


def run_one(s: specmod.Spec, args, cfg: dict, out_root: Path) -> dict:
    from . import target as targetmod

    extra_env = dict(kv.split("=", 1) for kv in args.target_env)
    t = targetmod.make(args.target, args.ephemeral, extra_env)
    r: dict = {
        "test": s.key,
        "title": s.title,
        "os": t.os,
        "issues": s.issues,
        "build": args.build or cfg["default_build"],
        "checks": [],
        "mode": "live",
        "verdict": "fail",
        "cost_usd": 0.0,
    }
    if t.os not in s.os:
        r.update(verdict="skip", mode="-", error=f"{s.key} does not run on {t.os}")
        return r
    out = out_root / s.area / s.name / t.os
    out.mkdir(parents=True, exist_ok=True)
    traj = trajectory.trajectory_dir(s.area, s.name, t.os)
    have_traj = (traj / "portable.json").exists()
    mode = args.mode
    if mode == "auto":
        mode = "replay" if have_traj else "live"
    if mode == "replay" and not have_traj:
        r.update(
            verdict="fail",
            mode="replay",
            error="no trajectory for this OS; run live with --record first",
        )
        return r

    attempt = run_attempt(s, args, cfg, t, out, mode, traj)
    if mode == "replay" and attempt.get("replay_broke") and args.mode == "auto":
        broke = attempt["replay_broke"]
        print(f"replay broke ({broke}); falling back to a live run", flush=True)
        attempt = run_attempt(s, args, cfg, t, out, "live", traj)
        attempt["replay_broke"] = broke
    r.update(attempt)
    return r


def run_attempt(s, args, cfg, t, out: Path, mode: str, traj: Path) -> dict:
    run_id = time.strftime("%m%d%H%M") + secrets.token_hex(2) + t.os[0]
    attempt_out = out / mode
    attempt_out.mkdir(parents=True, exist_ok=True)
    relay_base = cfg["endpoints"]["CUA_RELAY_URL"]
    res: dict = {
        "mode": mode,
        "checks": [],
        "verdict": "fail",
        "model": cfg["model"] if mode == "live" else "-",
    }
    m = None
    dialogs = None
    try:
        with tempfile.TemporaryDirectory(prefix="agentic-e2e-") as tmp:
            m = provision.Machine(
                t, s, run_id, cfg, args.driver_bin, attempt_out, args.build or cfg["default_build"]
            )
            m.start_driver()
            dialogs = checks.DialogWatcher(m, cfg.get("watched_dialogs") or [])
            dialogs.start()
            m.install = provision.install(t, m.build, m.workdir, Path(tmp))
            m.note(f"installed {m.install.version} on {t.label}")
            m.session = canary.Session()
            m.setup_hosts()
            variables = m.variables()

            event_watchers = {
                i: checks.EventUiWatcher(m, c, variables)
                for i, c in enumerate(s.expect)
                if c["type"] == "ui" and c.get("after_event")
            }
            for w in event_watchers.values():
                w.start()

            if mode == "replay":
                try:
                    n = trajectory.replay(m.mcp, traj, m.note, variables)
                    m.note(f"replayed {n} turns")
                except trajectory.ReplayBroke as e:
                    res["replay_broke"] = str(e)
                    return res
            else:
                record_dir = t.path(m.workdir, "recording")
                prompt = specmod.render(s.prompt, variables)
                context = host_context(m)
                run = agent.run_agent(
                    prompt=prompt,
                    context=context,
                    os_name=t.os,
                    model=cfg["model"],
                    effort=cfg["effort"],
                    max_steps=s.steps,
                    minutes=s.minutes,
                    run_dir=attempt_out,
                    driver_argv=m.driver_argv(),
                    record_dir=record_dir,
                    bare=bool(os.environ.get("ANTHROPIC_API_KEY")) and args.ephemeral,
                )
                res.update(
                    agent_exit=run.exit,
                    agent_text=run.final_text,
                    cost_usd=run.cost_usd,
                    turns=run.turns,
                    steps=run.steps,
                    seconds=run.seconds,
                )
                t.fetch(record_dir, attempt_out / "recording")

            variables = m.variables()
            results = checks.run_checks(m, s.expect, variables, dialogs, event_watchers, relay_base)
            res["checks"] = [c.__dict__ for c in results]
            passed = all(c.ok for c in results)
            if mode == "replay" and not passed:
                res["replay_broke"] = "checks failed after replay: " + "; ".join(
                    c.name for c in results if not c.ok
                )
                return res
            res["verdict"] = "pass" if passed else "fail"
            if passed and mode == "live" and args.record:
                rec = attempt_out / "recording"
                portable = trajectory.build_portable(
                    attempt_out, rec, specmod.render(s.prompt, variables), variables
                )
                trajectory.save(
                    traj,
                    rec,
                    portable,
                    {
                        "test": s.key,
                        "os": t.os,
                        "model": cfg["model"],
                        "build": m.build,
                        "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                        "run_url": os.environ.get("CUA_E2E_RUN_URL", ""),
                        "steps": res.get("steps", 0),
                    },
                )
                m.note(
                    f"saved trajectory ({'replayable' if portable['replayable'] else 'not replayable: ' + portable['not_replayable_because']})"
                )
            if args.review and mode == "live":
                res["review"] = review(cfg, attempt_out, res)
    except Exception as e:
        res["error"] = f"{type(e).__name__}: {e}"
        res["verdict"] = "error"
        traceback.print_exc()
    finally:
        if dialogs:
            dialogs.stop()
            res["dialogs_seen"] = dialogs.seen
        if m is not None:
            try:
                problems = m.teardown(relay_base)
                if problems:
                    res["teardown_problems"] = problems
            except Exception as e:
                res["teardown_problems"] = [f"{type(e).__name__}: {e}"]
            res["setup_log"] = m.log
            if m.session:
                m.session.logout()
        report.write(
            attempt_out,
            {
                **res,
                "test": s.key,
                "title": s.title,
                "os": t.os,
                "issues": s.issues,
                "build": args.build or cfg["default_build"],
            },
        )
    return res


def host_context(m: provision.Machine) -> str:
    parts = []
    for n, h in sorted(m.hosts.items()):
        if h.spec.role == "app":
            parts.append(
                "The Cua Spaces app is open on this computer, signed in to a test account."
            )
        if h.spec.terminal:
            parts.append(
                f"The terminal window titled {h.spec.terminal!r} is a shell on another computer "
                f"(test computer {n}); `cua` and `cua-e2e-power off|on` (turns that computer off or on) work there."
            )
    return " ".join(parts)


def review(cfg: dict, out: Path, res: dict) -> str:
    """Advisory only: a short read of the run by a model. Never sets the verdict."""
    calls = (
        (out / "calls.jsonl").read_text(encoding="utf-8")[-30000:]
        if (out / "calls.jsonl").exists()
        else ""
    )
    prompt = (
        "Review this end-to-end test run of a desktop app. In at most 5 bullets: did the agent reach the goal "
        "through the UI, anything odd the app showed, and whether the hard checks look meaningful. "
        f"Checks: {json.dumps(res.get('checks'))}\nAgent said: {res.get('agent_text', '')}\nTool calls (JSONL):\n{calls}"
    )
    p = subprocess.run(
        [
            "claude",
            "-p",
            prompt,
            "--model",
            cfg["review_model"],
            "--tools",
            "",
            "--strict-mcp-config",
            "--setting-sources",
            "",
            "--no-session-persistence",
            "--output-format",
            "json",
        ],
        capture_output=True,
        text=True,
        timeout=300,
    )
    try:
        return json.loads(p.stdout).get("result", "")
    except ValueError:
        return "(review failed)"


def cmd_run(args) -> int:
    cfg = load_config()
    out_root = Path(args.out)
    out_root.mkdir(parents=True, exist_ok=True)
    results = []
    for p in resolve(args.specs):
        s = specmod.load(p)
        print(f"== {s.key}", flush=True)
        results.append(run_one(s, args, cfg, out_root))
    ran = [r for r in results if r["verdict"] != "skip"]
    text = report.summary(ran)
    (out_root / "summary.md").write_text(text, encoding="utf-8")
    (out_root / "results.json").write_text(json.dumps(ran, indent=2, default=str), encoding="utf-8")
    print(text)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as f:
            f.write(text)
    return 0 if ran and all(r["verdict"] == "pass" for r in ran) else 1


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(
        prog="bin/agentic-e2e",
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = ap.add_subparsers(dest="cmd", required=True)
    v = sub.add_parser("validate")
    v.add_argument("specs", nargs="*")
    v.set_defaults(fn=cmd_validate)
    li = sub.add_parser("list")
    li.add_argument("--os")
    li.set_defaults(fn=cmd_list)
    se = sub.add_parser("select")
    se.add_argument("--changed", required=True)
    se.add_argument("--os")
    se.set_defaults(fn=cmd_select)
    r = sub.add_parser("run")
    r.add_argument("specs", nargs="*")
    r.add_argument("--mode", choices=["live", "replay", "auto"], default="auto")
    r.add_argument("--target", default="local")
    r.add_argument("--target-env", action="append", default=[])
    r.add_argument("--ephemeral", action="store_true")
    r.add_argument("--build")
    r.add_argument("--driver-bin", default="cua-driver")
    r.add_argument("--record", action="store_true")
    r.add_argument("--review", action="store_true")
    r.add_argument("--out", default=str(REPO / "artifacts" / "agentic-e2e"))
    r.set_defaults(fn=cmd_run)
    args = ap.parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
