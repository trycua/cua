"""Runs a code block of a provider's documentation page UNMODIFIED.

    docs_snippet.py MDX_FILE "## Heading text" LANG [--nth N] [--prelude daytona] [--keep-sandbox]

Finds the heading, takes the Nth (default 1st) fenced block of LANG (python or
typescript) under it, and runs it. Docs blocks often assume a `sandbox`
variable from the page before: `--prelude daytona` first creates one the way
the page does (`daytona = Daytona(); sandbox = daytona.create()`) and deletes
it afterwards. Written by us; the block comes from the fetched, pinned page.
Exits with the block's exit code."""

import os
import re
import subprocess
import sys

if os.environ.get("EX_IN_SPACE") != "1":
    sys.exit(
        "docs_snippet: runs third-party documentation code; only inside a job Space (EX_IN_SPACE=1)"
    )
mdx, heading, lang = sys.argv[1:4]
nth = int(sys.argv[sys.argv.index("--nth") + 1]) if "--nth" in sys.argv else 1
prelude = sys.argv[sys.argv.index("--prelude") + 1] if "--prelude" in sys.argv else ""

text = open(mdx, encoding="utf-8").read()
i = text.index(heading)
level = len(heading) - len(heading.lstrip("#"))
rest = text[i + len(heading) :]
# the section ends at the next heading of the same or a higher level (a `#` line
# inside a code fence is a comment, not a heading)
out, fence = [], False
for ln in rest.splitlines(keepends=True):
    if ln.lstrip().startswith("```"):
        fence = not fence
    elif not fence and re.match(r"#{1,%d} " % level, ln):
        break
    out.append(ln)
section = "".join(out)
blocks = re.findall(r"^[ \t]*```%s[^\n]*\n(.*?)^[ \t]*```" % re.escape(lang), section, re.M | re.S)
if len(blocks) < nth:
    sys.exit(f"docs_snippet: only {len(blocks)} {lang} block(s) under {heading!r}")
code = blocks[nth - 1]
# docs blocks are indented inside <TabItem>: remove the common indent
lines = code.splitlines()
indent = min((len(lnk) - len(lnk.lstrip()) for lnk in lines if lnk.strip()), default=0)
code = "\n".join(lnk[indent:] for lnk in lines) + "\n"

if lang == "python":
    pre = ""
    post = ""
    if prelude == "daytona":
        pre = "from daytona import Daytona as _D\n_daytona = _D()\nsandbox = _daytona.create()\n"
        post = "\ntry:\n    sandbox.delete()\nexcept Exception as _e:\n    print('cleanup:', _e)\n"
    src = pre + code + post
    open("snippet.py", "w").write(src)
    print("--- snippet.py ---\n" + src + "--- run ---", flush=True)
    sys.exit(subprocess.call([sys.executable, "snippet.py"]))
else:
    pre = ""
    post = ""
    if prelude == "daytona":
        pre = "import { Daytona as _D } from '@daytona/sdk';\nconst _daytona = new _D();\nconst sandbox = await _daytona.create();\n"
        post = "\ntry { await sandbox.delete() } catch (e) { console.log('cleanup:', String(e)) }\n"
    src = pre + code + post
    open("snippet.mts", "w").write(src)
    print("--- snippet.mts ---\n" + src + "--- run ---", flush=True)
    sys.exit(subprocess.call(["npx", "tsx", "snippet.mts"]))
