#!/usr/bin/env python3
"""Public examples suite: other providers' own examples, tests and popular
open-source integrations, run UNMODIFIED against Cua through
`cua run --compat <provider>`.

Nothing third-party is vendored. Each manifest entry names a repository, a
pinned commit and a path. This runner fetches that path at test time (git,
data only: no hooks, no filters, never executed on this machine), checks the
commit and the git tree id against the manifest, and ships the files plus
generated dependency lists to a job Space. The job installs its dependencies
(uv for Python, node for TypeScript) and runs the example there. The host
never installs or runs any of it.

    run.py lint                         check the manifest (offline)
    run.py lock [--ids a,b]             fetch, fill tree ids and license hashes
    run.py verify [--ids a,b]           fetch and check pins (no Space)
    run.py run [--provider p] [--ids a,b] [--sdk pinned|latest] [--out DIR]
    run.py summary DIR                  pass rates of a finished run

Result per example: DIR/<id>.json; DIR/summary.json has the pass rate per
provider. Statuses: pass; native-error (the example hit a provider feature
Cua cannot offer and the provider's own error came back, as the manifest
expects); fail (a bug: file it in compat-hardening/BUGS.md); infra (the job
could not run: not counted as a pass).
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from pathlib import Path

HERE = Path(__file__).resolve().parent
MANIFEST = HERE / "manifest.json"
PROVIDERS = ("e2b", "daytona", "modal")
KINDS = ("script", "pytest", "vitest", "driver")
# Licenses whose terms allow running unmodified code from a pinned fetch.
ALLOWED_SPDX = {
    "Apache-2.0",
    "MIT",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "MPL-2.0",
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
    "LGPL-3.0-or-later",
    "GPL-3.0-only",
}
LICENSE_TEXT = {
    "Apache-2.0": "Apache License",
    "MIT": "Permission is hereby granted",
    "AGPL-3.0-only": "GNU AFFERO GENERAL PUBLIC LICENSE",
    "BSD-3-Clause": "Redistribution and use",
    "BSD-2-Clause": "Redistribution and use",
    "MPL-2.0": "Mozilla Public License",
}
SHA40 = re.compile(r"^[0-9a-f]{40}$")
PYTEST_DEPS = [
    "pytest>=9.0.3,<10",
    "pytest-xdist>=3.3.1,<4",
    "pytest-dotenv>=0.5.2,<0.6",
    "pytest-asyncio>=1.3.0,<2",
    "pytest-timeout>=2.4.0,<3",
    "python-dotenv>=1.0.0,<2",
]
MARK_BEGIN = "@@EX-BEGIN"
MARK_END = "@@EX-END"
MARK_JUNIT = "@@EX-JUNIT"
MARK_INSTALL = "@@EX-INSTALL"


def load_manifest() -> dict:
    return json.loads(MANIFEST.read_text())


def select(manifest: dict, ids: str | None, provider: str | None) -> list[dict]:
    want = set(ids.split(",")) if ids else None
    out = []
    for e in manifest["entries"]:
        if want is not None and e["id"] not in want:
            continue
        if provider and e["provider"] != provider:
            continue
        out.append(e)
    return out


# ---------------------------------------------------------------------------
# lint


def lint(manifest: dict) -> list[str]:
    errs: list[str] = []
    seen: set[str] = set()
    versions = json.loads((HERE.parent / "SDK_VERSIONS.json").read_text())
    for p in ("e2b", "daytona", "modal"):
        pf = HERE.parent / p / "PINNED"
        if not pf.is_file():
            continue
        pin = pf.read_text().strip()
        if versions[p]["py"]["pinned"] != pin:
            errs.append(
                f"SDK_VERSIONS.json {p}.py.pinned {versions[p]['py']['pinned']} != libs/cua/tests/compat/{p}/PINNED {pin}"
            )
    for e in manifest["entries"]:
        i = e.get("id", "?")

        def bad(msg: str, i=i) -> None:
            errs.append(f"{i}: {msg}")

        if i in seen:
            bad("duplicate id")
        seen.add(i)
        if not re.fullmatch(r"[a-z0-9][a-z0-9._-]*", i):
            bad("id must be lowercase [a-z0-9._-]")
        if e.get("provider") not in PROVIDERS:
            bad(f"provider {e.get('provider')!r}")
        if e.get("lang") not in ("py", "ts"):
            bad("lang must be py or ts")
        if e.get("kind") not in KINDS:
            bad(f"kind {e.get('kind')!r}")
        src = e.get("source") or {}
        if not src.get("repo", "").startswith("https://github.com/"):
            bad("source.repo must be an https://github.com/ URL")
        if not SHA40.match(src.get("commit", "")):
            bad("source.commit must be a 40-hex commit sha")
        if not src.get("path"):
            bad("source.path missing")
        lic = e.get("license") or {}
        if lic.get("spdx") not in ALLOWED_SPDX:
            bad(
                f"license {lic.get('spdx')!r} not allowed (no license or a license that forbids use is dropped)"
            )
        if not lic.get("evidence"):
            bad("license.evidence missing (where the license was read)")
        if not lic.get("file"):
            bad("license.file missing (path of the LICENSE file at the commit)")
        if e.get("needs_secrets"):
            bad(
                "needs_secrets must be empty: only examples that run without third-party keys are kept"
            )
        if e.get("needs_llm") not in (False, "mock"):
            bad("needs_llm must be false or 'mock'")
        run = e.get("run") or {}
        if not isinstance(run.get("cmd"), list) or not run["cmd"]:
            bad("run.cmd must be a non-empty list")
        ex = e.get("expect") or {}
        if not ex:
            bad("expect missing")
        if not e.get("sdk_specs"):
            bad("sdk_specs missing (the provider SDK packages this entry installs, pinned)")
        if e["kind"] == "driver" and not (HERE / "drivers" / e.get("driver", "")).is_file():
            bad(f"driver file {e.get('driver')!r} not in drivers/")
        kf = e.get("known_failure")
        if kf is not None and not (kf.get("ref") and kf.get("reason")):
            bad("known_failure needs ref (a BUGS.md entry) and reason")
        if e.get("tree") is not None and not SHA40.match(e["tree"]):
            bad("tree must be a 40-hex git tree id")
    return errs


# ---------------------------------------------------------------------------
# fetch (host side, data only)


def git(args: list[str], cwd: Path, check: bool = True) -> str:
    env = {
        "PATH": os.environ.get("PATH", ""),
        "HOME": str(cwd),  # no user config, no credential helpers
        "GIT_TERMINAL_PROMPT": "0",
        "GIT_LFS_SKIP_SMUDGE": "1",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": "/dev/null",
    }
    base = [
        "git",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "protocol.file.allow=never",
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "filter.lfs.smudge=",
        "-c",
        "filter.lfs.process=",
        "-c",
        "filter.lfs.required=false",
    ]
    r = subprocess.run(base + args, cwd=cwd, env=env, capture_output=True, text=True)
    if check and r.returncode != 0:
        raise RuntimeError(f"git {' '.join(args[:3])}: {r.stderr.strip()[:400]}")
    return r.stdout


def fetch_repo(cache: Path, repo: str, commit: str) -> Path:
    """A partial, depth-1 checkout object store for repo@commit (no files yet)."""
    d = cache / (
        re.sub(r"[^a-z0-9]+", "-", repo.lower().removeprefix("https://github.com/"))
        + "-"
        + commit[:12]
    )
    if not (d / ".git").is_dir():
        d.mkdir(parents=True, exist_ok=True)
        git(["init", "-q"], d)
        git(["remote", "add", "origin", repo], d)
        git(["fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", commit], d)
    head = git(["rev-parse", "FETCH_HEAD"], d).strip()
    if head != commit:
        raise RuntimeError(f"{repo}: fetched {head}, manifest pins {commit}")
    return d


def tree_id(d: Path, commit: str, path: str) -> str:
    return git(["rev-parse", f"{commit}:{path}"], d).strip()


def read_blob(d: Path, commit: str, path: str) -> bytes:
    r = subprocess.run(
        ["git", "-c", "core.hooksPath=/dev/null", "show", f"{commit}:{path}"],
        cwd=d,
        capture_output=True,
        env={
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(d),
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_LFS_SKIP_SMUDGE": "1",
        },
    )
    if r.returncode != 0:
        raise RuntimeError(f"{path}@{commit[:8]}: {r.stderr.decode()[:200]}")
    return r.stdout


def materialize(d: Path, commit: str, path: str, dest: Path) -> int:
    """Write the files of `path` (a file or a directory) at `commit` into dest.
    Uses git archive, so no working-tree filters or hooks apply."""
    dest.mkdir(parents=True, exist_ok=True)
    r = subprocess.run(
        ["git", "-c", "core.hooksPath=/dev/null", "archive", "--format=tar", commit, "--", path],
        cwd=d,
        capture_output=True,
        env={
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(d),
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_LFS_SKIP_SMUDGE": "1",
        },
    )
    if r.returncode != 0:
        raise RuntimeError(f"git archive {path}: {r.stderr.decode()[:300]}")
    import io
    import tarfile

    n = 0
    with tarfile.open(fileobj=io.BytesIO(r.stdout)) as t:
        for m in t.getmembers():
            if m.issym() or m.islnk() or m.isdev():
                continue  # never materialize links or devices from third-party trees
            name = os.path.normpath(m.name)
            if name.startswith("..") or os.path.isabs(name):
                raise RuntimeError(f"unsafe path in archive: {m.name}")
            target = dest / name
            if m.isdir():
                target.mkdir(parents=True, exist_ok=True)
            elif m.isfile():
                target.parent.mkdir(parents=True, exist_ok=True)
                f = t.extractfile(m)
                assert f is not None
                target.write_bytes(f.read())
                target.chmod(0o755 if m.mode & 0o111 else 0o644)
                n += 1
    return n


def compile_check(root: Path) -> list[str]:
    """Syntax-check fetched Python files without importing or running them."""
    bad = []
    for f in root.rglob("*.py"):
        try:
            compile(f.read_text(encoding="utf-8", errors="replace"), str(f), "exec")
        except SyntaxError as ex:
            bad.append(f"{f.relative_to(root)}: {ex}")
    return bad


def cmd_verify(manifest: dict, args: argparse.Namespace, lock: bool = False) -> int:
    cache = Path(args.cache)
    bad = 0
    entries = select(manifest, args.ids, args.provider)
    for e in entries:
        s = e["source"]
        try:
            d = fetch_repo(cache, s["repo"], s["commit"])
            tid = tree_id(d, s["commit"], s["path"])
            lf = e["license"]["file"]
            lblob = read_blob(d, s["commit"], lf)
            lsha = hashlib.sha256(lblob).hexdigest()
            needle = LICENSE_TEXT.get(e["license"]["spdx"])
            if needle and needle.lower() not in lblob.decode("utf-8", "replace").lower():
                raise RuntimeError(
                    f"{lf} does not look like {e['license']['spdx']} (no {needle!r})"
                )
            if lock:
                e["tree"] = tid
                e["license"]["file_sha256"] = lsha
                status = "locked"
            else:
                if e.get("tree") != tid:
                    raise RuntimeError(f"tree id {tid} != manifest {e.get('tree')}")
                if e["license"].get("file_sha256") != lsha:
                    raise RuntimeError("license file changed at the pinned commit?!")
                status = "ok"
            if args.compile and e["lang"] == "py":
                tmp = Path(tempfile.mkdtemp(prefix="vc-", dir=cache))
                try:
                    materialize(d, s["commit"], s["path"], tmp)
                    errs = compile_check(tmp)
                    if errs:
                        raise RuntimeError("does not compile: " + "; ".join(errs[:3]))
                finally:
                    shutil.rmtree(tmp, ignore_errors=True)
            print(
                f"{status:7} {e['id']}  {s['repo'].split('/')[-1]}@{s['commit'][:10]}:{s['path']}"
            )
        except Exception as ex:  # noqa: BLE001
            bad += 1
            print(f"FAIL    {e['id']}: {ex}")
    if lock:
        MANIFEST.write_text(json.dumps(manifest, indent=2) + "\n")
    return 1 if bad else 0


# ---------------------------------------------------------------------------
# build the job directory


def sdk_spec(e: dict, manifest: dict, which: str) -> list[str]:
    """Package specs of the provider SDK(s) for this entry; `latest` drops
    the version pins (the weekly drift lane)."""
    specs = list(e["sdk_specs"])
    if which == "latest":
        out = []
        for s in specs:
            if e["lang"] == "py":
                out.append(re.split(r"[=<>~!]", s, maxsplit=1)[0])
            else:
                out.append(re.sub(r"@[^@/]+$", "", s) if "@" in s[1:] else s)
        return out
    return specs


def build_job(e: dict, manifest: dict, cache: Path, job: Path, sdk: str) -> dict:
    s = e["source"]
    d = fetch_repo(cache, s["repo"], s["commit"])
    tid = tree_id(d, s["commit"], s["path"])
    if e.get("tree") and e["tree"] != tid:
        raise RuntimeError(
            f"{e['id']}: git tree id {tid} != manifest {e['tree']} (pin check failed)"
        )
    src = job / "src"
    n = materialize(d, s["commit"], s["path"], src)
    # run.cwd is relative to the repository root (src/ holds repo-relative paths)
    default_cwd = s["path"] if (src / s["path"]).is_dir() else os.path.dirname(s["path"])
    for extra in e.get("also_fetch", []):  # shared files outside the path (a conftest, a lockfile)
        materialize(d, s["commit"], extra, src)
        n += 1
    if e["kind"] == "driver":
        shutil.copy(HERE / "drivers" / e["driver"], job / e["driver"])
    for f in e.get("our_files", []):  # helper files written by us (mock servers)
        shutil.copy(HERE / "drivers" / f, job / f)
    py = list(sdk_spec(e, manifest, sdk)) if e["lang"] == "py" else list(e.get("py_deps", []))
    py += e.get("extra_deps", []) if e["lang"] == "py" else []
    if e["kind"] == "pytest":
        py += e.get("pytest_deps", PYTEST_DEPS)
    (job / "requirements.txt").write_text("\n".join(py) + "\n")
    if e["run"].get("install"):  # the entry installs its own tree (a pnpm workspace)
        (job / "entry.install").write_text(e["run"]["install"] + "\n")
    elif e["lang"] == "ts":
        deps = list(sdk_spec(e, manifest, sdk)) + list(e.get("extra_deps", []))
        if e["kind"] == "vitest":
            deps += e.get("vitest_deps", ["vitest@3.2.4"])
        deps.append("tsx@4.20.6")
        dd = {}
        for dep in deps:
            name, _, ver = dep.rpartition("@") if "@" in dep[1:] else (dep, "", "")
            dd[name or dep] = ver or "latest"
        (job / "package.json").write_text(
            json.dumps({"name": "ex", "private": True, "dependencies": dd}, indent=2)
        )
    run = e["run"]
    cwd = run.get("cwd", default_cwd) or "."
    job_cwd = cwd == "@job"  # driver entries run from the job root
    rel_root = "src"
    cmd = " ".join(
        shlex.quote(c) if not re.search(r"\{(job|src|junit)\}", c) else c for c in run["cmd"]
    )
    cmd = (
        cmd.replace("{job}", "$JOB")
        .replace("{src}", "$JOB/src")
        .replace("{junit}", "$JOB/junit.xml")
    )
    env = {
        "EX_ID": e["id"],
        "EX_LANG": e["lang"],
        "EX_KIND": e["kind"],
        "EX_CWD": "." if job_cwd else os.path.normpath(f"{rel_root}/{cwd}"),
        "EX_TIMEOUT": str(e.get("timeout_s", 600)),
        "EX_MOCK_LLM": "1" if e.get("needs_llm") == "mock" else "0",
    }
    (job / "entry.env").write_text(
        "".join(f"export {k}={shlex.quote(v)}\n" for k, v in env.items())
    )
    (job / "entry.cmd").write_text(cmd + "\n")
    for k, v in (e.get("env") or {}).items():
        with (job / "entry.env").open("a") as f:
            f.write(f"export {k}={shlex.quote(str(v))}\n")
    with (job / "entry.env").open("a") as f:
        for k in e.get("env_unset", []):  # client-side unit tests that assume a clean environment
            f.write(f"unset {shlex.quote(k)}\n")
    shutil.copy(HERE / "inner" / "run_entry.sh", job / "run_entry.sh")
    (job / "run_entry.sh").chmod(0o755)
    if e["needs_llm"] == "mock" or e.get("needs_llm") == "mock":
        shutil.copy(HERE / "drivers" / "mock_openai.py", job / "mock_openai.py")
    # a git repo so `cua run` ships exactly these files
    git(["init", "-q"], job)
    git(["add", "-A"], job)
    # `cua run` takes a bundle of at most 1 MiB compressed; say so before submitting
    import io
    import tarfile

    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as t:
        for f in git(["ls-files"], job).splitlines():
            t.add(job / f, arcname=f)
    kb = buf.tell() // 1024
    if kb > 1000:
        raise RuntimeError(
            f"{e['id']}: the job bundle is {kb} KB compressed; `cua run` takes at most 1024 KB"
        )
    return {"files": n, "tree": tid, "bundle_kb": kb}


# ---------------------------------------------------------------------------
# run a job and judge


def parse_output(text: str) -> dict:
    """Pull the example's output, exit code, install status and junit out of
    the job's log."""
    out: dict = {"stdout": "", "exit": None, "junit": None, "install": None}
    lines = text.splitlines()
    buf: list[str] = []
    inside = False
    jb = []
    for ln in lines:
        m = re.search(r"(@@EX-[A-Z]+)(.*)$", ln)
        if m and m.group(1) == MARK_BEGIN:
            inside, buf = True, []
            continue
        if m and m.group(1) == MARK_END:
            inside = False
            em = re.search(r"exit=(-?\d+)", m.group(2))
            out["exit"] = int(em.group(1)) if em else None
            continue
        if m and m.group(1) == MARK_JUNIT:
            jb.append(m.group(2).strip())
            continue
        if m and m.group(1) == MARK_INSTALL:
            out["install"] = m.group(2).strip()
            continue
        if inside:
            buf.append(ln)
    out["stdout"] = "\n".join(buf)
    if jb:
        try:
            out["junit"] = base64.b64decode("".join(jb)).decode("utf-8", "replace")
        except Exception:  # noqa: BLE001
            out["junit"] = None
    return out


def junit_summary(xml_text: str) -> dict:
    root = ET.fromstring(xml_text)
    res = {"passed": 0, "failed": [], "skipped": 0}
    for tc in root.iter("testcase"):
        name = f"{tc.get('classname', '')}::{tc.get('name', '')}".strip(":")
        kids = {c.tag for c in tc}
        if "failure" in kids or "error" in kids:
            msg = ""
            for c in tc:
                if c.tag in ("failure", "error"):
                    msg = (c.get("message") or c.text or "")[:300]
            res["failed"].append({"test": name, "message": msg})
        elif "skipped" in kids:
            res["skipped"] += 1
        else:
            res["passed"] += 1
    return res


def judge(e: dict, parsed: dict) -> tuple[str, list[str]]:
    ex = e["expect"]
    why: list[str] = []
    stdout = parsed["stdout"]
    if parsed["exit"] is None:
        return "infra", [
            "the example did not run to completion (no end marker); install: %s" % parsed["install"]
        ]
    if parsed["install"] not in (None, "ok"):
        return "infra", [f"dependency install failed: {parsed['install']}"]
    native = ex.get("native_error")
    if native:
        # The example needs something the provider has and Cua cannot give: it must
        # fail with the provider's own error, not succeed and not die otherwise.
        if parsed["exit"] == 0 and not native.get("exit_zero_ok"):
            why.append("expected the provider-native error but the example succeeded")
        for needle in native.get("contains", []):
            if needle not in stdout:
                why.append(f"native error text {needle!r} not in output")
        return ("native-error" if not why else "fail"), why
    if e["kind"] in ("pytest", "vitest"):
        if not parsed["junit"]:
            return "fail", ["no junit report came back"]
        js = junit_summary(parsed["junit"])
        allowed = {a["test"] if isinstance(a, dict) else a for a in ex.get("allowed_failures", [])}
        unexpected = [
            f
            for f in js["failed"]
            if not any(f["test"].endswith(a) or a in f["test"] for a in allowed)
        ]
        if unexpected:
            why.append(
                f"{len(unexpected)} unexpected failure(s): "
                + "; ".join(f["test"] for f in unexpected[:8])
            )
        if js["passed"] < ex.get("min_passed", 1):
            why.append(f"only {js['passed']} passed, expected at least {ex.get('min_passed', 1)}")
        parsed["junit_summary"] = js
    else:
        want = ex.get("exit_code", 0)
        if want == "nonzero":
            if parsed["exit"] == 0:
                why.append("expected a nonzero exit")
        elif parsed["exit"] != want:
            why.append(f"exit {parsed['exit']} != {want}")
    for needle in ex.get("stdout_contains", []):
        if needle not in stdout:
            why.append(f"missing in output: {needle!r}")
    for rx in ex.get("stdout_regex", []):
        if not re.search(rx, stdout, re.M):
            why.append(f"no match for /{rx}/")
    for needle in ex.get("stdout_not_contains", []):
        if needle in stdout:
            why.append(f"unexpected in output: {needle!r}")
    return ("pass" if not why else "fail"), why


def run_entry(e: dict, manifest: dict, a: argparse.Namespace, out: Path) -> dict:
    t0 = time.time()
    res: dict = {
        "id": e["id"],
        "provider": e["provider"],
        "lang": e["lang"],
        "kind": e["kind"],
        "source": {k: e["source"][k] for k in ("repo", "commit", "path")},
        "sdk": a.sdk,
    }
    scratch = Path(tempfile.mkdtemp(prefix=f"ex-{e['id']}-", dir=a.scratch))
    try:
        info = build_job(e, manifest, Path(a.cache), scratch, a.sdk)
        res.update(info)
        if a.dry:
            res["status"] = "built"
            return res
        cmd = [a.cua, "run", "--compat", e["provider"]]
        if a.kind:
            cmd += ["--kind", a.kind]
        if a.on:
            cmd += ["--on", a.on]
        cmd += ["--cpus", str(e.get("cpus", a.cpus)), "--memory", str(e.get("memory_mb", a.memory))]
        for k in e.get("pass_env", []):
            cmd += ["--env", k]
        cmd += ["--", "sh", "run_entry.sh"]
        res["cmd"] = " ".join(cmd)
        for attempt in range(1, a.busy_retries + 2):
            p = subprocess.run(
                cmd,
                cwd=scratch,
                capture_output=True,
                text=True,
                timeout=e.get("timeout_s", 600) + 900,
            )
            text = p.stdout + "\n" + p.stderr
            res["attempts"] = attempt
            # The shared test machine can be too loaded to place a Space (see BUGS.md,
            # core), or a dependency install can fail on a full disk: wait and run
            # the whole entry again; the attempt count is recorded in the result.
            transient = (
                "has not enough CPU free" in text
                or "not enough disk space" in text
                or ("@@EX-INSTALL failed" in text and "Failed to" in text)
            )
            if transient and attempt <= a.busy_retries:
                time.sleep(a.busy_wait)
                continue
            break
        (out / f"{e['id']}.log").write_text(text[-400000:])
        parsed = parse_output(text)
        res["cua_exit"] = p.returncode
        res["exit"] = parsed["exit"]
        res["install"] = parsed["install"]
        res["status"], res["why"] = judge(e, parsed)
        kf = e.get("known_failure")
        if kf:
            res["known_failure"] = kf
            if res["status"] == "fail":
                res["status"] = "known-failure"  # filed in BUGS.md; does not fail the lane
            elif res["status"] == "pass":
                res["status"], res["why"] = "fail", [
                    f"listed as a known failure ({kf.get('ref')}) but it passes: remove known_failure from the manifest"
                ]
        if res["status"] == "infra" and p.returncode != 0:
            res["why"].append("cua run: " + (p.stderr.strip().splitlines() or ["failed"])[-1][:300])
        res["output_tail"] = parsed["stdout"][-3000:]
        if "junit_summary" in parsed:
            res["junit"] = parsed["junit_summary"]
    except subprocess.TimeoutExpired:
        res["status"], res["why"] = "infra", ["cua run timed out"]
    except Exception as ex:  # noqa: BLE001
        res["status"], res["why"] = "infra", [f"{type(ex).__name__}: {ex}"]
    finally:
        res["seconds"] = round(time.time() - t0, 1)
        if not a.keep:
            shutil.rmtree(scratch, ignore_errors=True)
    return res


def summarize(results: list[dict], manifest: dict) -> dict:
    per: dict = {}
    for r in results:
        p = per.setdefault(
            r["provider"],
            {"total": 0, "pass": 0, "native-error": 0, "known-failure": 0, "fail": 0, "infra": 0},
        )
        p["total"] += 1
        p[r["status"]] = p.get(r["status"], 0) + 1
    for p in per.values():
        ok = p["pass"] + p["native-error"]
        p["pass_rate"] = round(ok / p["total"], 3) if p["total"] else None
    return {"providers": per, "entries": len(results), "manifest_entries": len(manifest["entries"])}


def warm(provider: str, a: argparse.Namespace) -> None:
    """One long-timeout create + delete through the provider's job gateway."""
    if provider not in ("e2b", "daytona"):
        return
    job = Path(tempfile.mkdtemp(prefix=f"warm-{provider}-", dir=a.scratch))
    try:
        shutil.copy(HERE / "drivers" / "warm.py", job / "warm.py")
        git(["init", "-q"], job)
        git(["add", "-A"], job)
        cmd = [
            a.cua,
            "run",
            "--compat",
            provider,
            "--kind",
            a.kind,
            "--on",
            a.on,
            "--cpus",
            "1",
            "--memory",
            "1024",
            "--",
            "python3",
            "warm.py",
            provider,
        ]
        for _ in range(a.busy_retries + 1):
            p = subprocess.run(cmd, cwd=job, capture_output=True, text=True, timeout=1500)
            if "has not enough CPU free" in p.stdout + p.stderr:
                time.sleep(a.busy_wait)
                continue
            break
        tail = (p.stdout + p.stderr).strip().splitlines()[-2:]
        print(f"   warm-up {provider}: " + " | ".join(tail), flush=True)
    except Exception as ex:  # noqa: BLE001
        print(f"   warm-up {provider} skipped: {ex}", flush=True)
    finally:
        shutil.rmtree(job, ignore_errors=True)


def cmd_run(manifest: dict, a: argparse.Namespace) -> int:
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    Path(a.cache).mkdir(parents=True, exist_ok=True)
    Path(a.scratch).mkdir(parents=True, exist_ok=True)
    entries = select(manifest, a.ids, a.provider)
    if a.lane:
        entries = [e for e in entries if a.lane in e.get("lanes", ["nightly"])]
    if a.shard:  # K/N: greedy balance by the entries' timeout (a weight proxy), stable
        k, n = (int(x) for x in a.shard.split("/"))
        loads, mine = [0] * n, []
        for e in sorted(entries, key=lambda e: (-e.get("timeout_s", 600), e["id"])):
            i = loads.index(min(loads))
            loads[i] += e.get("timeout_s", 600)
            if i == k - 1:
                mine.append(e)
        entries = [e for e in entries if e in mine]
    results = []
    warmed: set[str] = set()
    for e in entries:
        if not a.no_warm and not a.dry and e["provider"] not in warmed:
            warmed.add(e["provider"])
            warm(e["provider"], a)
        print(f"== {e['id']} ({e['provider']} {e['lang']} {e['kind']})", flush=True)
        # The shared test Mac sometimes runs out of system-wide file handles (ENFILE):
        # wait and retry instead of losing the run.
        for tries in range(8):
            try:
                r = run_entry(e, manifest, a, out)
                if (
                    r["status"] == "infra"
                    and any(
                        "Too many open files" in w or "Permission denied" in w
                        for w in r.get("why", [])
                    )
                    and tries < 7
                ):
                    time.sleep(45)
                    continue
                (out / f"{e['id']}.json").write_text(json.dumps(r, indent=2) + "\n")
                break
            except OSError as ex:
                print(f"   host error ({ex}); waiting", flush=True)
                time.sleep(45)
        else:
            continue
        print(
            f"   {r['status']}  {r.get('seconds')}s  {'; '.join(r.get('why', []))[:300]}",
            flush=True,
        )
        results.append(r)
    s = summarize(results, manifest)
    (out / "summary.json").write_text(json.dumps(s, indent=2) + "\n")
    print(json.dumps(s, indent=2))
    bad = [r for r in results if r["status"] in ("fail", "infra")]
    return 1 if bad and not a.allow_fail else 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("cmd", choices=["lint", "lock", "verify", "run", "summary"])
    ap.add_argument("dir", nargs="?")
    ap.add_argument("--ids")
    ap.add_argument("--provider", choices=PROVIDERS)
    ap.add_argument("--sdk", choices=["pinned", "latest"], default="pinned")
    ap.add_argument("--lane", help="only entries in this lane (pr|nightly)")
    base = Path(os.environ.get("CUA_EXAMPLES_WORK", tempfile.gettempdir())) / "cua-examples"
    ap.add_argument("--cache", default=str(base / "cache"))
    ap.add_argument("--scratch", default=str(base / "scratch"))
    ap.add_argument("--out", default=str(base / "out"))
    ap.add_argument("--cua", default=os.environ.get("CUA", "cua"))
    ap.add_argument("--kind", default=os.environ.get("EXAMPLES_KIND", "container"))
    ap.add_argument("--on", default=os.environ.get("EXAMPLES_ON", "host:local"))
    ap.add_argument("--cpus", type=float, default=2.0)
    ap.add_argument("--memory", type=int, default=4096)
    ap.add_argument(
        "--busy-retries", type=int, default=int(os.environ.get("EXAMPLES_BUSY_RETRIES", "0"))
    )
    ap.add_argument("--busy-wait", type=int, default=60)
    ap.add_argument("--shard", help="K/N: run shard K of N (several machines in parallel)")
    ap.add_argument("--no-warm", action="store_true", help="skip the warm-up create")
    ap.add_argument(
        "--compile", action="store_true", help="verify: also syntax-check fetched Python"
    )
    ap.add_argument("--keep", action="store_true", help="keep job directories")
    ap.add_argument("--dry", action="store_true", help="build job directories only")
    ap.add_argument("--allow-fail", action="store_true")
    a = ap.parse_args()
    m = load_manifest()
    if a.cmd == "lint":
        errs = lint(m)
        for x in errs:
            print("LINT", x)
        print(f"{len(m['entries'])} entries, {len(errs)} problems")
        return 1 if errs else 0
    if a.cmd == "verify":
        return cmd_verify(m, a)
    if a.cmd == "lock":
        return cmd_verify(m, a, lock=True)
    if a.cmd == "summary":
        rs = [
            json.loads(p.read_text())
            for p in sorted(Path(a.dir).glob("*.json"))
            if p.name != "summary.json"
        ]
        print(json.dumps(summarize(rs, m), indent=2))
        return 0
    return cmd_run(m, a)


if __name__ == "__main__":
    sys.exit(main())
