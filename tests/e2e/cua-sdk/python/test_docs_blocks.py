"""docs-blocks: every guide code block tagged ``test="<lane>"`` runs here
(extracted by ../docs/extract.py), so the guides cannot rot silently.

* ``docs``: hermetic blocks (Python, and TypeScript through tsx). Python
  blocks of one session run cumulatively (each sees the session's earlier
  blocks, like a reader following the page); the session is the page unless
  a block names one with ``session="..."``. A block whose ``print(...)`` is
  followed by a ``# {...}`` / ``# [...]`` comment must print that literal.
* ``container``: the same runner in the container lane (local sandboxes on
  Docker / gVisor), for ``local=True`` examples.
* ``fleet``: whole scripts against live Fleet (nightly), with
  ``CUA_POOL_NAME`` set to ``cua-e2e-<run>-docs-<n>``; the pool is deleted
  afterwards even when the guide intentionally leaves it warm.
* ``terraform``: ``terraform init/validate`` of the HCL (needs terraform).
* ``contrib``: blocks on contrib providers (``on="daytona"``) against
  ``cua-contrib-fixtures`` (schema mocks of the provider APIs, not recorded
  traffic, and a mock cua-spacesd). Needs ``CUA_CONTRIB_FIXTURES`` and a
  cua binding built with ``--features contrib``; the contrib CI lane
  provides both.

Hidden preludes (``prelude="a,b"``) set the block up without showing it:
``fakefleet`` points the SDK at the fake Fleet API of cua-test-fixtures
(``CUA_FLEET_BASE_URL`` / ``FLEETS_TOKEN``) so cloud examples run on PRs
unchanged; ``spacesd`` exposes the fixtures' MockServer spacesd as
``CUA_DOCS_SPACESD_URL`` / ``CUA_DOCS_SPACESD_TOKEN``; a file
``../docs/preludes/<name>.py`` (or ``.ts``) is prepended to the program.
Every block runs with a temporary ``HOME`` / ``CUA_HOME``, never the
reader's real one.

Python blocks run in a venv holding the repo's ``cua-sandbox[driver,mcp]``
(the package and extras the guides use), created once per session.
"""

from __future__ import annotations

import ast
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import e2e
import pytest

import cua

sys.path.insert(0, str(e2e.SUITE / "docs"))
import extract  # noqa: E402

BLOCKS = extract.all_blocks()
SENTINEL = "---cua-e2e-docs-block---"
PRELUDES = e2e.SUITE / "docs" / "preludes"
TS_LANGS = ("ts", "typescript")
# Credentials a hermetic block must never see.
LIVE_CREDENTIALS = (
    "CUA_CLIENT_ID",
    "CUA_CLIENT_SECRET",
    "FLEETS_TOKEN",
    "CUA_API_KEY",
    # Contrib provider keys: hermetic blocks talk to the fixtures only.
    "E2B_API_KEY",
    "DAYTONA_API_KEY",
)


@pytest.fixture(scope="session")
def sandbox_python(tmp_path_factory) -> str:
    """A venv with the repo's cua-sandbox[driver,mcp] (editable) and the cua SDK source."""
    uv = shutil.which("uv")
    if not uv:
        pytest.skip("uv is required for docs blocks")
    venv = tmp_path_factory.mktemp("docs-venv") / "venv"
    subprocess.run([uv, "venv", "-q", "--python", "3.12", str(venv)], check=True, timeout=300)
    py = str(venv / "bin" / "python")
    # The repo's cua SDK package (with its staged native library) first, so
    # cua-sandbox's `cua>=0.2.0` resolves to it rather than PyPI.
    out = subprocess.run(
        [
            uv,
            "pip",
            "install",
            "-q",
            "--python",
            py,
            "-e",
            str(e2e.REPO / "libs" / "cua" / "python"),
            "-e",
            # `[driver]`: the guides that drive a desktop import cua_driver
            # (cua-driver pinned by cua-sandbox's extra, as users install it).
            # `[mcp]`: the MCP how-to calls `sb.mcp()` (the official MCP SDK).
            f"{e2e.REPO / 'libs' / 'python' / 'cua-sandbox'}[driver,mcp]",
        ],
        capture_output=True,
        text=True,
        timeout=900,
    )
    if out.returncode != 0:
        pytest.skip(f"cannot install cua-sandbox: {out.stderr[-500:]}")
    return py


def _env(py: str, extra: dict | None = None) -> dict:
    env = dict(os.environ)
    env["PYTHONPATH"] = os.pathsep.join(
        [str(e2e.REPO / "libs" / "cua" / "python" / "src"), env.get("PYTHONPATH", "")]
    ).rstrip(os.pathsep)
    env.update(extra or {})
    return env


def _expected_literal(code: str):
    """The literal in the `# ...` comment right after the block's only print."""
    lines = code.splitlines()
    prints = [i for i, ln in enumerate(lines) if ln.lstrip().startswith("print(")]
    if len(prints) != 1:
        return None
    comment = []
    for ln in lines[prints[0] + 1 :]:
        if not ln.lstrip().startswith("#"):
            break
        comment.append(ln.lstrip()[1:].strip())
    text = " ".join(comment)
    if not text or text[0] not in "{[" or "..." in text:
        return None
    try:
        return ast.literal_eval(text)
    except (ValueError, SyntaxError):
        return None


def _tail(stderr: str) -> str:
    """The error, not the interpreter-shutdown noise after it."""
    lines = [
        ln
        for ln in stderr.splitlines()
        if "__del__" not in ln and "uniffi_cua_sdk_fn_free" not in ln
    ]
    return "\n".join(lines)[-4000:]


def _ids(blocks):
    return [b.id for b in blocks]


DOCS = [b for b in BLOCKS if "docs" in b.lanes and b.lang == "python"]
CONTAINER = [b for b in BLOCKS if "container" in b.lanes and b.lang == "python"]
DOCS_TS = [b for b in BLOCKS if "docs" in b.lanes and b.lang in TS_LANGS]
CONTAINER_TS = [b for b in BLOCKS if "container" in b.lanes and b.lang in TS_LANGS]
FLEET_PY = [b for b in BLOCKS if "fleet" in b.lanes and b.lang == "python"]
FLEET_TS = [b for b in BLOCKS if "fleet" in b.lanes and b.lang in TS_LANGS]
TERRAFORM = [b for b in BLOCKS if "terraform" in b.lanes]
CONTRIB = [b for b in BLOCKS if "contrib" in b.lanes and b.lang == "python"]


def _preludes(block) -> list[str]:
    return [p.strip() for p in block.prelude.split(",") if p.strip()]


# Preludes that only point a block at fixtures: the live fleet lane drops them.
HERMETIC_PRELUDES = {"fakefleet", "spacesd", "space", "space-url", "spaces"}


def _prelude_code(block, ext: str, live: bool = False) -> str:
    parts = []
    for name in _preludes(block):
        if live and name in HERMETIC_PRELUDES:
            continue
        f = PRELUDES / f"{name}.{ext}"
        if f.exists():
            parts.append(f.read_text())
    return "\n".join(parts)


@pytest.fixture
def docs_fixtures():
    """A fresh cua-test-fixtures per block: a docs block's fake Fleet state
    (managed pools, claims) never leaks into other tests, or other blocks."""
    binary = e2e.fixtures_binary()
    if binary is None:
        pytest.skip("cua-test-fixtures is not built")
    proc = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("cua-test-fixtures exited before printing endpoints")
        yield json.loads(line)
    finally:
        proc.stdin.close()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=10)


@pytest.fixture
def contrib_fixtures():
    """A fresh cua-contrib-fixtures per block (schema mocks of the provider
    APIs and a mock cua-spacesd); it reports sandboxes a block leaked."""
    binary = os.environ.get("CUA_CONTRIB_FIXTURES")
    if not binary:
        pytest.skip("CUA_CONTRIB_FIXTURES is not set (the contrib lane builds it)")
    proc = subprocess.Popen(
        [binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    try:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("cua-contrib-fixtures exited before printing endpoints")
        yield json.loads(line)
    finally:
        try:
            _, stderr = proc.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            _, stderr = proc.communicate(timeout=10)
        assert "sandboxes left at exit" not in (stderr or ""), stderr


_DOCKER_HOST: list[str] = []


def _docker_host() -> str:
    if not _DOCKER_HOST:
        host = os.environ.get("DOCKER_HOST", "")
        if not host and shutil.which("docker"):
            out = subprocess.run(
                ["docker", "context", "inspect", "--format", "{{.Endpoints.docker.Host}}"],
                capture_output=True,
                text=True,
                timeout=30,
            )
            host = out.stdout.strip() if out.returncode == 0 else ""
        _DOCKER_HOST.append(host)
    return _DOCKER_HOST[0]


def _hermetic_env(block, request, home: Path) -> dict:
    """A temp HOME, no live credentials, and what the block's preludes need."""
    home.mkdir(parents=True, exist_ok=True)
    env = {
        "HOME": str(home),
        "CUA_HOME": str(home / ".cua"),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_DATA_HOME": str(home / ".local" / "share"),
        "CUA_NO_DAEMON_AUTOSTART": "1",
        # The names a page asks the reader to pick (preludes and .subst.json
        # read them); the fleet lane sets per-run names instead.
        "CUA_POOL_NAME": "cua-e2e-docs-pool",
        "CUA_CLAIM_NAME": "cua-e2e-docs-claim",
    }
    # The temp HOME hides the engine's socket discovery (~/.colima, ...):
    # hand the real one over explicitly.
    docker = _docker_host()
    if docker:
        env["DOCKER_HOST"] = docker
    names = set(_preludes(block))
    if "contrib" in names:
        fx = request.getfixturevalue("contrib_fixtures")
        env["DAYTONA_API_URL"] = fx["daytona_api"]
        env["DAYTONA_API_KEY"] = fx["daytona_key"]
        # No registry here: images run as tagged.
        env["CUA_IMAGE_RESOLVE"] = "0"
    if names & {"fakefleet", "spacesd", "space", "space-url", "spaces"}:
        fx = request.getfixturevalue("docs_fixtures")
        if "fakefleet" in names:
            env["CUA_FLEET_BASE_URL"] = fx["fleet_base_url"]
            env["FLEETS_TOKEN"] = fx["fleet_token"]
            # Picking gVisor or KubeVirt for an image reads its manifest, as
            # for a reader (run.py turns inspection off), from the fixtures'
            # registry mirror: no network, no Docker Hub rate limit.
            env["CUA_FLEET_IMAGE_INSPECT"] = "1"
            env["CUA_REGISTRY_MIRRORS"] = fx["registry_mirror"]
        if "spacesd" in names:
            env["CUA_DOCS_SPACESD_URL"] = fx["env_url"]
            env["CUA_DOCS_SPACESD_TOKEN"] = fx["env_token"]
        if names & {"space", "space-url"}:
            # A real cua-spacesd server core confined to temp dirs; teleport
            # reads the fixtures' synthetic Firefox profile, never the host's.
            env["CUA_DOCS_SPACE_URL"] = fx["spaces_url"]
            env["CUA_DOCS_SPACE_TOKEN"] = fx["spaces_token"]
            env["CUA_SPACES_TELEPORT_HOME"] = fx["teleport_host_home"]
            env["CUA_SPACES_AGENT_CREDENTIALS_HOME"] = "none"
    return env


def _substitute(block, code: str, env: dict) -> str:
    """preludes/<name>.subst.json maps a literal the page shows (a reader's
    address or token) to the env var holding the fixture's value."""
    for name in _preludes(block):
        f = PRELUDES / f"{name}.subst.json"
        if f.exists():
            for literal, var in json.loads(f.read_text()).items():
                if literal.startswith("$"):
                    continue
                code = code.replace(literal, json.dumps(env[var]))
    return code


# Runs a program that may use top-level `await` (a notebook-style excerpt that
# continues a hidden prelude); plain scripts run unchanged.
_BOOTSTRAP = """
import ast, asyncio, sys
path = sys.argv[1]
src = open(path).read()
try:
    code = compile(src, path, "exec")
    exec(code, {"__name__": "__main__"})
except SyntaxError as e:
    if "await" not in str(e.msg) and "async" not in str(e.msg):
        raise
    code = compile(src, path, "exec", flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT)
    result = eval(code, {"__name__": "__main__"})
    if asyncio.iscoroutine(result):
        asyncio.run(result)
"""


def _run_python(block, blocks, sandbox_python, request, tmp_path):
    # Only a named session is cumulative; blocks without one stand alone
    # (otherwise every earlier block on the page, and its sandbox, reruns).
    session = [
        b
        for b in blocks
        if block.session
        and b.guide == block.guide
        and b.session == block.session
        and b.index < block.index
    ]
    extra = _hermetic_env(block, request, tmp_path / "home")
    program = "\n".join(
        [_prelude_code(block, "py"), *(b.code for b in session), f"print({SENTINEL!r})", block.code]
    )
    program = _substitute(block, program, extra)
    script = tmp_path / "block.py"
    script.write_text(program)
    env = _env(sandbox_python, extra)
    for k in LIVE_CREDENTIALS:
        if k not in extra:
            env.pop(k, None)
    out = subprocess.run(
        [sandbox_python, "-c", _BOOTSTRAP, str(script)],
        capture_output=True,
        text=True,
        timeout=600,
        env=env,
        cwd=tmp_path,
    )
    assert out.returncode == 0, f"{block.guide}:{block.line}\n{_tail(out.stderr)}"
    expected = _expected_literal(block.code)
    if expected is not None:
        printed = out.stdout.split(SENTINEL, 1)[1].strip().splitlines()[-1]
        assert (
            ast.literal_eval(printed) == expected
        ), f"{block.guide}:{block.line}: printed {printed}"


def _tsx() -> list[str]:
    pinned = e2e.REPO / "docs" / "node_modules" / "tsx" / "dist" / "cli.mjs"
    if pinned.exists():
        return ["node", str(pinned)]
    if not shutil.which("npx"):
        pytest.skip("npx is required")
    return ["npx", "--yes", "tsx@4.21.0"]


def _ts_project(tmp_path: Path) -> Path:
    """A module dir where `@trycua/cua` resolves to the repo package."""
    pkg = e2e.CUA_ROOT / "typescript"
    if not (pkg / "dist" / "index.js").exists():
        pytest.skip("@trycua/cua is not built (libs/cua/typescript: npm run build)")
    tmp_path.mkdir(parents=True, exist_ok=True)
    (tmp_path / "package.json").write_text('{"type":"module"}')
    (tmp_path / "node_modules" / "@trycua").mkdir(parents=True, exist_ok=True)
    (tmp_path / "node_modules" / "@trycua" / "cua").symlink_to(pkg)
    return tmp_path


def _run_ts(block, request, tmp_path, extra_env: dict | None = None):
    project = _ts_project(tmp_path / "project")
    script = project / "block.ts"
    code = "\n".join([_prelude_code(block, "ts", live=extra_env is not None), block.code])
    code = _substitute(
        block,
        code,
        extra_env if extra_env is not None else _hermetic_env(block, request, tmp_path / "home"),
    )
    script.write_text(code)
    if extra_env is None:
        extra_env = _hermetic_env(block, request, tmp_path / "home")
        env = dict(os.environ, **extra_env)
        for k in LIVE_CREDENTIALS:
            if k not in extra_env:
                env.pop(k, None)
    else:
        env = dict(os.environ, **extra_env)
    out = subprocess.run(
        [*_tsx(), str(script)],
        capture_output=True,
        text=True,
        timeout=600,
        cwd=project,
        env=env,
    )
    assert out.returncode == 0, f"{block.guide}:{block.line}\n{_tail(out.stderr)}"


def test_guides_are_tagged():
    """Every guide this suite owns still carries at least one runnable block."""
    tagged = {b.guide for b in BLOCKS}
    missing = [g for g in extract.GUIDES if g not in tagged]
    assert not missing, f"untagged guides: {missing}"


test_guides_are_tagged = pytest.mark.e2e("docs-blocks", "docs")(test_guides_are_tagged)


@pytest.mark.e2e("docs-blocks", "docs")
@pytest.mark.parametrize("block", DOCS, ids=_ids(DOCS))
def test_docs_block(block, sandbox_python, request, tmp_path):
    _run_python(block, DOCS, sandbox_python, request, tmp_path)


# Lanes without a tagged block define no test (an empty parameter set would
# be a skip, which run.py --strict rejects).
if CONTAINER:

    @pytest.mark.e2e("docs-blocks", "container")
    @pytest.mark.parametrize("block", CONTAINER, ids=_ids(CONTAINER))
    def test_container_block(block, sandbox_python, request, tmp_path):
        _run_python(block, CONTAINER, sandbox_python, request, tmp_path)


if DOCS_TS:

    @pytest.mark.e2e("docs-blocks", "docs")
    @pytest.mark.parametrize("block", DOCS_TS, ids=_ids(DOCS_TS))
    def test_docs_ts_block(block, request, tmp_path):
        _run_ts(block, request, tmp_path)


if CONTAINER_TS:

    @pytest.mark.e2e("docs-blocks", "container")
    @pytest.mark.parametrize("block", CONTAINER_TS, ids=_ids(CONTAINER_TS))
    def test_container_ts_block(block, request, tmp_path):
        _run_ts(block, request, tmp_path)


def _delete_pool(pool: str) -> None:
    async def body():
        try:
            await cua.embedded(fleet_from_env=True).fleet().delete_pool(pool)
        except cua.CuaError:
            pass

    e2e.run_async(body(), timeout=300)


@pytest.mark.e2e("docs-blocks", "fleet")
@pytest.mark.parametrize("block", FLEET_PY, ids=_ids(FLEET_PY))
def test_fleet_script(block, sandbox_python, tmp_path):
    # Inputs the guide asks the reader to export (e.g. OMARCHY_IMAGE, an
    # image this repo does not publish); the harness sets only the pool names.
    needed = sorted(
        set(re.findall(r"""os\.environ\[["']([A-Z0-9_]+)["']\]""", block.code))
        - {"CUA_POOL_NAME", "CUA_CLAIM_NAME"}
    )
    missing = [n for n in needed if not os.environ.get(n)]
    if missing:
        pytest.skip(f"docs block input {', '.join(missing)} is unset")
    pool = f"cua-e2e-{e2e.RUN}-docs-{block.index}"[:63]
    names = {"CUA_POOL_NAME": pool, "CUA_CLAIM_NAME": f"{pool}-claim"}
    # The same preludes as on PRs, minus the ones that point at fixtures, so
    # a fragment runs live exactly as it runs against the fake Fleet.
    program = "\n".join([_prelude_code(block, "py", live=True), block.code])
    script = tmp_path / (block.title or "block.py")
    script.write_text(_substitute(block, program, names))
    try:
        out = subprocess.run(
            [sandbox_python, "-c", _BOOTSTRAP, str(script)],
            capture_output=True,
            text=True,
            timeout=2700,
            cwd=tmp_path,
            env=_env(sandbox_python, names),
        )
        print(out.stdout[-2000:])
        assert out.returncode == 0, f"{block.guide}:{block.line}\n{out.stderr[-3000:]}"
    finally:
        _delete_pool(pool)


@pytest.mark.e2e("docs-blocks", "fleet")
# The TypeScript guide page (create-pool-with-typescript) was removed upstream;
# when no Fleet TS block is tagged, report that instead of an empty parameter set.
@pytest.mark.parametrize(
    "block",
    FLEET_TS
    or [
        pytest.param(
            None, marks=pytest.mark.skip(reason="no Fleet TypeScript docs block is tagged")
        )
    ],
    ids=_ids(FLEET_TS) if FLEET_TS else ["none"],
)
def test_fleet_ts_script(block, request, tmp_path):
    """A TypeScript guide script against live Fleet, with @trycua/cua from
    the repo (libs/cua/typescript) and tsx."""
    pool = f"cua-e2e-{e2e.RUN}-docs-ts-{block.index}"[:63]
    try:
        _run_ts(
            block, request, tmp_path, {"CUA_POOL_NAME": pool, "CUA_CLAIM_NAME": f"{pool}-claim"}
        )
    finally:
        _delete_pool(pool)


if CONTRIB:

    @pytest.mark.e2e("docs-blocks", "contrib")
    @pytest.mark.parametrize("block", CONTRIB, ids=_ids(CONTRIB))
    def test_contrib_block(block, sandbox_python, request, tmp_path):
        _run_python(block, CONTRIB, sandbox_python, request, tmp_path)


@pytest.mark.e2e("docs-blocks", "docs")
@pytest.mark.parametrize("block", TERRAFORM, ids=_ids(TERRAFORM))
def test_terraform_block(block, tmp_path):
    tf = shutil.which("terraform") or shutil.which("tofu")
    if not tf:
        pytest.skip("terraform (or tofu) is not installed")
    (tmp_path / (block.title or "main.tf")).write_text(block.code)
    for args in (["init", "-backend=false", "-input=false"], ["validate"]):
        out = subprocess.run([tf, *args], capture_output=True, text=True, timeout=600, cwd=tmp_path)
        assert out.returncode == 0, f"terraform {args[0]}: {_tail(out.stderr)}"
