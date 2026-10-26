"""Our driver for crewai-tools' E2B sandbox tools (crewAIInc/crewAI,
lib/crewai-tools/.../e2b_sandbox_tool): the public tool API, no LLM. Written
by us; crewai-tools and e2b are the unmodified PyPI packages. Prints PASS
<check> per check and a final PASS."""

import base64
import os
import sys

if os.environ.get("EX_IN_SPACE") != "1":
    sys.exit("runs only inside a job Space (EX_IN_SPACE=1)")

from crewai_tools import E2BExecTool, E2BFileTool

failed = []


def check(name, cond, detail=""):
    print(
        ("PASS " if cond else "FAIL ") + name + (f" ({detail})" if detail and not cond else ""),
        flush=True,
    )
    if not cond:
        failed.append(name)


# one sandbox for the file tool (persistent), ephemeral for the exec tool
files = E2BFileTool(persistent=True)
try:
    r = files.run(action="write", path="/home/user/crew/hello.txt", content="hello crew\n")
    check("write creates parent dirs", r.get("status") in ("written", "ok") or "path" in r, r)
    r = files.run(action="read", path="/home/user/crew/hello.txt")
    check("read round trip", r.get("content") == "hello crew\n", r)
    files.run(action="append", path="/home/user/crew/hello.txt", content="more\n")
    check(
        "append",
        files.run(action="read", path="/home/user/crew/hello.txt").get("content")
        == "hello crew\nmore\n",
    )
    raw = bytes(range(256))
    files.run(
        action="write",
        path="/home/user/crew/bin.dat",
        content=base64.b64encode(raw).decode(),
        binary=True,
    )
    back = files.run(action="read", path="/home/user/crew/bin.dat", binary=True)
    check("binary round trip", base64.b64decode(back["content"]) == raw)
    ls = files.run(action="list", path="/home/user/crew")
    names = str(ls)
    check("list shows both files", "hello.txt" in names and "bin.dat" in names, names[:200])
    check(
        "exists true",
        files.run(action="exists", path="/home/user/crew/hello.txt").get("exists") is True,
    )
    files.run(action="delete", path="/home/user/crew/hello.txt")
    check(
        "exists false after delete",
        files.run(action="exists", path="/home/user/crew/hello.txt").get("exists") is False,
    )
finally:
    files.close()

ex = E2BExecTool()
r = ex.run(command="echo hi; echo err >&2", envs={"X": "1"})
check(
    "exec ok",
    r.get("exit_code") == 0
    and r.get("stdout", "").strip() == "hi"
    and "err" in (r.get("stderr") or ""),
    r,
)
r = ex.run(command="echo $X-$PWD", cwd="/tmp", envs={"X": "7"})
check("cwd and envs honoured", (r.get("stdout") or "").strip() == "7-/tmp", r)
try:
    r = ex.run(command="exit 3")
    check(
        "non-zero exit surfaces as the E2B exception (or exit_code 3)", r.get("exit_code") == 3, r
    )
except Exception as e:  # CommandExitException carries the exit code in the real SDK
    check(
        "non-zero exit surfaces as the E2B exception (or exit_code 3)",
        getattr(e, "exit_code", None) == 3,
        repr(e),
    )

print("PASS" if not failed else "FAILED: " + ", ".join(failed))
sys.exit(1 if failed else 0)
