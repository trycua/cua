"""Our driver for the e2b-code-interpreter quickstart (README of
e2b-dev/code-interpreter): the calls a first-time user makes, with the
observable results asserted. Written by us; the SDK is the unmodified
e2b-code-interpreter package. Prints PASS <check> per check and a final PASS."""

import os
import sys

if os.environ.get("EX_IN_SPACE") != "1":
    sys.exit("runs only inside a job Space (EX_IN_SPACE=1)")

from e2b_code_interpreter import Sandbox

failed = []


def check(name, cond, detail=""):
    print(
        ("PASS " if cond else "FAIL ") + name + (f" ({detail})" if detail and not cond else ""),
        flush=True,
    )
    if not cond:
        failed.append(name)


with Sandbox.create(timeout=300) as sbx:
    sbx.run_code("x = 1")
    ex = sbx.run_code("x += 1; x")
    check("state persists across run_code", ex.text == "2", ex.text)

    out = []
    ex = sbx.run_code("print('a'); print('b')", on_stdout=lambda m: out.append(m.line))
    check(
        "stdout callback order",
        out == ["a\n", "b\n"] or out == ["a", "b"] or "".join(out).replace("\n", "") == "ab",
        repr(out),
    )
    check(
        "logs.stdout collected",
        "".join(ex.logs.stdout).replace("\n", "") == "ab",
        repr(ex.logs.stdout),
    )

    ex = sbx.run_code("1/0")
    check(
        "error name", ex.error is not None and ex.error.name == "ZeroDivisionError", repr(ex.error)
    )

    ctx = sbx.create_code_context()
    sbx.run_code("y = 41", context=ctx)
    check("context isolation", sbx.run_code("y + 1", context=ctx).text == "42")
    ex = sbx.run_code("y")
    check(
        "default context does not see y",
        ex.error is not None and ex.error.name == "NameError",
        repr(ex.error),
    )

    ex = sbx.run_code("console.log('js-ok'); 6*7", language="js")
    check("javascript kernel", "js-ok" in "".join(ex.logs.stdout), repr(ex))

    ex = sbx.run_code(
        "import matplotlib\nmatplotlib.use('Agg')\nimport matplotlib.pyplot as plt\nplt.plot([1,2,3])\nplt.show()"
    )
    check(
        "plot returns a result",
        len(ex.results) >= 1 and ex.results[0].png is not None,
        repr(ex.error),
    )

    check(
        "filesystem + commands share the sandbox",
        sbx.commands.run("echo shell-ok").stdout.strip() == "shell-ok",
    )

print("PASS" if not failed else "FAILED: " + ", ".join(failed))
sys.exit(1 if failed else 0)
