"""The code interpreter (contexts, streaming, errors, timeouts) and chart
extraction of `process.code_run`. Need a real Python in the guest.

Derived from daytona 0.225.0 `_sync/code_interpreter.py`,
`common/code_interpreter.py`, `common/charts.py`."""

from harness import case, err_obs


@case(
    "interpreter.default_context_keeps_state",
    needs="space",
    ref="_sync/code_interpreter.py run_code (default shared context)",
)
async def default_state(c):
    sb = await c.main()
    ci = sb.code_interpreter
    r = await c.w(ci.run_code("x = 41\nimport math\ndef f(a): return a + 1"))
    assert r.error is None and r.stdout == "" and r.stderr == "", r
    r = await c.w(ci.run_code("print(f(x), math.floor(2.7))"))
    assert r.stdout == "42 2\n" and r.error is None, r
    return {"stdout": "42 2\n"}


@case(
    "interpreter.contexts_are_isolated",
    needs="space",
    ref="_sync/code_interpreter.py create_context/list_contexts/delete_context",
)
async def contexts(c):
    sb = await c.main()
    ci = sb.code_interpreter
    a = await c.w(ci.create_context())
    b = await c.w(ci.create_context(cwd="/tmp"))
    try:
        await c.w(ci.run_code("v = 'a'", context=a))
        await c.w(ci.run_code("v = 'b'", context=b))
        ra = await c.w(ci.run_code("print(v)", context=a))
        rb = await c.w(ci.run_code("print(v)", context=b))
        assert (ra.stdout, rb.stdout) == ("a\n", "b\n"), (ra, rb)
        rc = await c.w(ci.run_code("import os; print(os.getcwd())", context=b))
        assert rc.stdout.strip() == "/tmp", rc
        miss = await c.w(ci.run_code("print(v)"))  # the default context has no v
        assert miss.error is not None and miss.error.name == "NameError", miss
        ids = {x.id for x in await c.w(ci.list_contexts())}
        assert {a.id, b.id} <= ids and "default" not in ids, ids
        assert (a.language, b.cwd, a.active) == ("python", "/tmp", True), (a, b)
    finally:
        await c.w(ci.delete_context(a))
        await c.w(ci.delete_context(b))
    gone = await c.raises(lambda: ci.delete_context(a))
    assert gone and gone["status_code"] == 404, gone
    ids = {x.id for x in await c.w(ci.list_contexts())}
    assert not ({a.id, b.id} & ids)
    return {"isolated": True, "cwd": "/tmp", "defaultNotListed": True, "deleteTwice": err_obs(gone)}


@case(
    "interpreter.streams_stdout_stderr_and_errors",
    needs="space",
    ref="_sync/code_interpreter.py run_code (on_stdout, on_stderr, on_error)",
)
async def streaming(c):
    sb = await c.main()
    ci = sb.code_interpreter
    out, err, errs = [], [], []
    code = "import sys, time\nfor i in range(3):\n    print('o%d' % i, flush=True)\n    time.sleep(0.2)\nsys.stderr.write('warn\\n')\nraise ValueError('boom')\n"
    r = await c.w(
        ci.run_code(
            code,
            on_stdout=lambda m: out.append(m.output),
            on_stderr=lambda m: err.append(m.output),
            on_error=lambda e: errs.append(e),
        )
    )
    assert "".join(out) == "o0\no1\no2\n" and len(out) >= 2, out
    assert "".join(err) == "warn\n", err
    assert r.stdout == "o0\no1\no2\n" and r.stderr == "warn\n"
    assert r.error is not None and (r.error.name, r.error.value) == ("ValueError", "boom"), r.error
    assert (
        "ValueError: boom" in r.error.traceback and "Traceback" in r.error.traceback
    ), r.error.traceback
    assert len(errs) == 1 and errs[0].name == "ValueError"
    syn = await c.w(ci.run_code("def (:"))
    assert syn.error is not None and syn.error.name == "SyntaxError", syn.error
    return {
        "stdout": "o0\no1\no2\n",
        "stderr": "warn\n",
        "error": ["ValueError", "boom"],
        "syntax": "SyntaxError",
        "streamedInChunks": True,
    }


@case("interpreter.envs_and_exit", needs="space", ref="_sync/code_interpreter.py run_code (envs)")
async def envs(c):
    sb = await c.main()
    ci = sb.code_interpreter
    r = await c.w(ci.run_code("import os; print(os.environ.get('K_ENV'))", envs={"K_ENV": "v1"}))
    assert r.stdout == "v1\n", r
    r = await c.w(ci.run_code("import os; print(os.environ.get('K_ENV'))"))
    assert r.stdout == "None\n", "an execution's envs do not outlive it"
    r = await c.w(ci.run_code("import subprocess; subprocess.run(['echo', 'from-child'])"))
    assert r.stdout == "from-child\n", r
    r = await c.w(ci.run_code("raise SystemExit(3)"))
    assert r.error is not None, r
    r = await c.w(ci.run_code("print('still alive')"))
    assert r.stdout == "still alive\n"
    return {"env": "v1", "restored": "None", "child": "from-child\n", "survivesExit": True}


@case(
    "interpreter.timeout",
    needs="space",
    ref="_sync/code_interpreter.py _maybe_raise_from_close (4008)",
)
async def timeout(c):
    sb = await c.main()
    ci = sb.code_interpreter
    e = await c.raises(
        lambda: ci.run_code("import time\nwhile True:\n    time.sleep(0.1)", timeout=1)
    )
    assert e and e["type"] == "DaytonaTimeoutError", e
    r = await c.w(ci.run_code("print('ok after timeout')"))
    assert r.stdout == "ok after timeout\n", r
    return {"timeout": err_obs(e), "usableAfter": True}


@case("process.code_run_charts", needs="space", ref="_sync/process.py code_run; common/charts.py")
async def charts(c):
    sb = await c.main()
    have = await c.w(
        sb.process.exec(
            "python3 -c 'import matplotlib' 2>&1 || python3 -m pip install --user --break-system-packages -q matplotlib 2>&1 | tail -1; python3 -c 'import matplotlib; print(1)' 2>&1 | tail -1"
        )
    )
    if have.result.strip().splitlines()[-1] != "1":
        return {"skipped": "no matplotlib in the guest"}
    code = """
import matplotlib.pyplot as plt
plt.figure(); plt.plot([1, 2, 3], [4, 5, 6], label='l1'); plt.title('Line'); plt.xlabel('X'); plt.ylabel('Y'); plt.show()
plt.figure(); plt.scatter([1, 2], [3, 4], label='pts'); plt.title('Scatter')
plt.figure(); plt.bar(['a', 'b'], [3, 5]); plt.title('Bars')
plt.figure(); plt.pie([30, 70], labels=['p', 'q']); plt.title('Pie')
plt.figure()
try:
    plt.boxplot([[1, 2, 3, 4, 100]], tick_labels=['one'])
except TypeError:
    plt.boxplot([[1, 2, 3, 4, 100]], labels=['one'])
plt.title('Box')
print('done')
"""
    r = await c.w(sb.process.code_run(code))
    assert r.exit_code == 0 and r.result == "done\n" and r.artifacts.stdout == "done\n", (
        r.exit_code,
        r.result,
    )
    kinds = [ch.type for ch in r.artifacts.charts]
    assert kinds == ["line", "scatter", "bar", "pie", "box_and_whisker"], kinds
    line = r.artifacts.charts[0]
    assert (line.title, line.x_label, line.y_label) == ("Line", "X", "Y"), line
    assert line.elements[0].label == "l1" and line.elements[0].points == [
        [1, 4],
        [2, 5],
        [3, 6],
    ], line.elements
    assert all(ch.png for ch in r.artifacts.charts)
    bar = r.artifacts.charts[2]
    assert [(e.label, e.value) for e in bar.elements] == [("a", "3"), ("b", "5")], bar.elements
    box = r.artifacts.charts[4].elements[0]
    assert (box.label, box.median, box.outliers) == ("one", 3, [100]), box
    none = await c.w(sb.process.code_run("print('no figures')"))
    assert none.artifacts.charts == [] and none.result == "no figures\n"
    return {
        "types": kinds,
        "line": [[1, 4], [2, 5], [3, 6]],
        "bars": [["a", "3"], ["b", "5"]],
        "boxMedian": 3,
        "outliers": [100],
    }
