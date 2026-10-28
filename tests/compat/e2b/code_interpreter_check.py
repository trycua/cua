"""e2b-code-interpreter against the environment's E2B endpoint (the real E2B,
or a gateway after `eval "$(cua compat env --provider e2b)"`).

Runs a fixed script through the SDK's public methods and prints a normalised
JSON observation; with a recording as the first argument it compares against
it (exit 1 on a difference). Record with `--record FILE` against real E2B.

    python code_interpreter_check.py [--record FILE | FILE]
"""

import json
import sys

from e2b_code_interpreter import Sandbox


def observe() -> dict:
    out: dict = {}
    sbx = Sandbox.create(timeout=120)
    try:
        r = sbx.run_code("x = 41\nprint('hello')\nimport sys\nprint('oops', file=sys.stderr)")
        out["print"] = {
            "stdout": r.logs.stdout,
            "stderr": r.logs.stderr,
            "error": r.error is not None,
        }
        r = sbx.run_code("x + 1")
        out["state"] = {
            "text": r.text,
            "results": [x.text for x in r.results],
            "main": [x.is_main_result for x in r.results],
        }
        r = sbx.run_code("1/0")
        out["error"] = {
            "name": r.error.name,
            "value": r.error.value,
            "has_traceback": "ZeroDivisionError" in r.error.traceback,
        }
        got = []
        sbx.run_code("print('a')\nprint('b')", on_stdout=lambda m: got.append(m.line))
        out["callback"] = got
        ctx = sbx.create_code_context(cwd="/tmp")
        sbx.run_code("v = 7", context=ctx)
        out["context"] = {
            "cwd": ctx.cwd,
            "language": ctx.language,
            "same": sbx.run_code("v", context=ctx).text,
            "default_isolated": sbx.run_code("v").error.name,
            "listed": any(c.id == ctx.id for c in sbx.list_code_contexts()),
            "pwd": sbx.run_code("import os; os.getcwd()", context=ctx).text,
        }
        sbx.restart_code_context(ctx)
        out["context"]["after_restart"] = sbx.run_code("v", context=ctx).error.name
        sbx.remove_code_context(ctx)
        out["context"]["listed_after_remove"] = any(
            c.id == ctx.id for c in sbx.list_code_contexts()
        )
        r = sbx.run_code("echo hi", language="bash")
        out["bash"] = {"stdout": r.logs.stdout, "error": r.error is not None}
        r = sbx.run_code("import os; os.environ.get('K')", envs={"K": "v"})
        out["envs"] = r.text
        out["files"] = sbx.commands.run("echo -n ok").stdout
    finally:
        sbx.kill()
    return out


def main() -> int:
    args = sys.argv[1:]
    obs = observe()
    if args[:1] == ["--record"]:
        open(args[1], "w").write(json.dumps(obs, indent=2, sort_keys=True) + "\n")
        return 0
    print(json.dumps(obs, indent=2, sort_keys=True))
    if args:
        want = json.load(open(args[0]))
        bad = {
            k: (want.get(k), obs.get(k)) for k in set(want) | set(obs) if want.get(k) != obs.get(k)
        }
        for k, (w, g) in sorted(bad.items()):
            print(f"DIFF {k}: recorded {w!r} got {g!r}", file=sys.stderr)
        return 1 if bad else 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
