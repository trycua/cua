"""The template area of the e2b module layout, and every build, tag and
lookup path, over the in-memory backend (no VMs, no host processes).

Recorded fixtures: `fixtures/dockerfile_recorded.json` holds the builder calls
(and errors) e2b 2.58.0's parser makes for each Dockerfile;
`fixtures/template_context_listing.json` the files e2b 2.58.0's
`get_all_files_in_path` selects for each source pattern and ignore list.
"""

from __future__ import annotations

import asyncio
import contextlib
import io
import json
import os
import tarfile
from datetime import datetime
from pathlib import Path

import e2b
import pytest
from e2b import AsyncSandbox
from e2b._backend import set_backend
from e2b.exceptions import BuildException, InvalidArgumentException, TemplateException
from e2b.template import _engine
from e2b.template import logger as logger_mod
from e2b.template import readycmd, utils
from e2b.template.dockerfile_parser import DockerfileSyntaxError, parse_dockerfile
from e2b.template.main import TemplateBase, TemplateBuilder, TemplateFinal
from e2b.template.types import (
    BuildInfo,
    InstructionType,
    TemplateBuildStatus,
    TemplateTag,
    TemplateTagInfo,
)
from e2b.template_async import build_api as async_api
from e2b.template_async.main import AsyncTemplate
from e2b.template_sync import build_api as sync_api
from e2b.template_sync.main import Template
from fake_backend import FakeBackend

FIXTURES = Path(__file__).parent / "fixtures"


@pytest.fixture
def backend(monkeypatch, tmp_path):
    monkeypatch.delenv("CUA_E2B_TEMPLATES", raising=False)
    monkeypatch.delenv("CUA_E2B_CPUS", raising=False)
    monkeypatch.delenv("CUA_E2B_MEMORY_MB", raising=False)
    monkeypatch.setattr(TemplateBase, "_logs_refresh_frequency", 0.01)
    b = FakeBackend()
    b.permissive = True
    set_backend(b)
    yield b
    set_backend(None)


def snapshots(backend) -> set:
    return {n for m in backend.machines.values() for n in m.cold_snapshots}


def builder_of(backend, snapshot: str):
    return next(m for m in backend.machines.values() if snapshot in m.cold_snapshots)


def ran(machine) -> list:
    return [s["argv"][-1] for s in machine.spawned]


def base_template(ctx=None) -> TemplateBuilder:
    return Template(file_context_path=ctx).from_base_image()


# ── the real module layout ─────────────────────────────────────────────────


def test_every_class_lives_in_the_module_e2b_has_it_in():
    from e2b.template import types

    assert TemplateBase.__module__ == "e2b.template.main"
    assert TemplateBuilder.__module__ == "e2b.template.main"
    assert TemplateFinal.__module__ == "e2b.template.main"
    assert AsyncTemplate.__module__ == "e2b.template_async.main"
    assert Template.__module__ == "e2b.template_sync.main"
    assert BuildInfo.__module__ == types.__name__ == "e2b.template.types"
    assert readycmd.ReadyCmd.__module__ == "e2b.template.readycmd"
    assert logger_mod.LogEntry.__module__ == "e2b.template.logger"
    assert logger_mod.DefaultBuildLogger.__module__ == "e2b.template.logger"
    assert DockerfileSyntaxError.__name__ == "DockerfileSyntaxError"
    # every public name stays importable from the places it was importable from
    for name in ("Template", "AsyncTemplate", "TemplateBase", "BuildInfo", "TemplateTag"):
        assert getattr(e2b, name) is not None
    assert e2b.template.TemplateBuilder is TemplateBuilder
    assert e2b.template_async.AsyncTemplate is AsyncTemplate
    assert e2b.template_sync.Template is Template
    assert issubclass(AsyncTemplate, TemplateBase) and issubclass(Template, TemplateBase)


def test_the_build_api_modules_have_e2bs_functions():
    names = {
        "request_build",
        "get_file_upload_link",
        "upload_file",
        "trigger_build",
        "get_build_status",
        "wait_for_build_finish",
        "check_alias_exists",
        "assign_tags",
        "remove_tags",
        "get_template_tags",
    }
    for mod in (async_api, sync_api):
        assert names <= set(dir(mod))
    assert asyncio.iscoroutinefunction(async_api.request_build)
    assert not asyncio.iscoroutinefunction(sync_api.request_build)


# ── ready commands, logs, types ────────────────────────────────────────────


def test_ready_commands_have_e2bs_shell_text():
    assert readycmd.wait_for_port(8080).get_cmd() == '[ -n "$(ss -Htuln sport = :8080)" ]'
    assert readycmd.wait_for_url("http://x/y z", 204).get_cmd() == (
        'curl -s -o /dev/null -w "%{http_code}" \'http://x/y z\' | grep -q "204"'
    )
    assert readycmd.wait_for_process("my d").get_cmd() == "pgrep 'my d' > /dev/null"
    assert readycmd.wait_for_file("/tmp/a b").get_cmd() == "[ -f '/tmp/a b' ]"
    assert readycmd.wait_for_timeout(500).get_cmd() == "sleep 1"
    assert readycmd.wait_for_timeout(5000).get_cmd() == "sleep 5"


def test_log_entries_strip_ansi_and_print_like_e2bs():
    now = datetime(2026, 1, 2, 3, 4, 5)
    entry = e2b.LogEntry(now, "info", "\x1b[31mred\x1b[0m text \x1b]0;title\x07")
    assert entry.message == "red text "
    assert str(entry) == "[2026-01-02T03:04:05] [info] red text "
    assert e2b.LogEntryStart(now, message="s").level == "debug"
    assert e2b.LogEntryEnd(timestamp=now, message="e").level == "debug"


def test_default_build_logger_filters_by_level(capsys):
    log = e2b.default_build_logger(min_level="warn")
    now = datetime(2026, 1, 2, 3, 4, 5)
    log(e2b.LogEntryStart(now, message="start"))
    log(e2b.LogEntry(now, "info", "hidden"))
    log(e2b.LogEntry(now, "warn", "shown"))
    log(e2b.LogEntry(now, "error", "also shown"))
    log(e2b.LogEntryEnd(now, message="end"))
    out = capsys.readouterr().out
    assert "hidden" not in out and "shown" in out and "WARN" in out and "ERROR" in out
    default = e2b.default_build_logger()
    default(e2b.LogEntry(now, "info", "info is on by default"))
    default(e2b.LogEntry(now, "debug", "debug is not"))
    out = capsys.readouterr().out
    assert "info is on by default" in out and "debug is not" not in out


def test_build_status_and_instruction_types_have_e2bs_values():
    assert [s.value for s in TemplateBuildStatus] == ["building", "waiting", "ready", "error"]
    assert TemplateBuildStatus.READY == "ready"
    assert {i.value for i in InstructionType} == {"COPY", "ENV", "RUN", "WORKDIR", "USER"}


# ── utils ──────────────────────────────────────────────────────────────────


def test_small_helpers():
    assert utils.pad_octal(0o755) == "0755" and utils.pad_octal(0o7) == "0007"
    assert utils.normalize_path("a" + os.sep + "b") == "a/b"
    assert utils.get_build_step_index("base", 9) == 0
    assert utils.get_build_step_index("finalize", 9) == 8
    assert utils.get_build_step_index("3", 9) == 3
    assert utils.normalize_build_arguments("a:v1", "old") == "a:v1"
    assert utils.normalize_build_arguments(None, "old") == "old"
    with pytest.raises(TemplateException, match="Name must be provided"):
        utils.normalize_build_arguments("", None)
    assert utils.strip_ansi_escape_codes("\x1b[1;32mok\x1b[0m\x1b[2K done") == "ok done"


@pytest.mark.parametrize("src", ["/abs/file", "../up", "a/../../up", "./a/../../../b", ".."])
def test_validate_relative_path_refuses_absolute_and_escaping_paths(src):
    with pytest.raises(TemplateException, match="Invalid source path"):
        utils.validate_relative_path(src, None)


@pytest.mark.parametrize("src", ["a", "a/b", "./a", "a/../b", "..config", "a/..b"])
def test_validate_relative_path_accepts_paths_inside_the_context(src):
    utils.validate_relative_path(src, None)


def test_the_traceback_points_at_the_user_call():
    def make():
        return utils.make_traceback(utils.get_caller_frame())

    tb = make()
    assert tb is not None and tb.tb_frame.f_code.co_filename == __file__
    assert utils.make_traceback(None) is None
    assert utils.get_caller_directory() == os.path.dirname(os.path.abspath(__file__))


def test_a_template_defaults_its_context_to_the_callers_directory(tmp_path):
    assert Template()._file_context_path == os.path.dirname(os.path.abspath(__file__))
    assert Template(file_context_path=tmp_path)._file_context_path == tmp_path.as_posix()
    assert Template(file_context_path="rel/dir")._file_context_path == "rel/dir"


def test_read_dockerignore_and_gcp_service_account(tmp_path):
    assert utils.read_dockerignore(str(tmp_path)) == []
    (tmp_path / ".dockerignore").write_text("# c\n\n*.log\n  node_modules  \n!keep.log\n")
    assert utils.read_dockerignore(str(tmp_path)) == ["*.log", "node_modules", "!keep.log"]
    (tmp_path / "sa.json").write_text('{"a": 1}')
    assert utils.read_gcp_service_account_json(str(tmp_path), "sa.json") == '{"a": 1}'
    assert json.loads(utils.read_gcp_service_account_json(str(tmp_path), {"b": 2})) == {"b": 2}


def _make_tree(root: Path, listing: dict):
    for rel in listing["tree"]:
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_text(rel)
    for link, target in listing["symlinks"].items():
        os.symlink(target, root / link)


def test_get_all_files_in_path_selects_what_e2b_selects(tmp_path):
    listing = json.loads((FIXTURES / "template_context_listing.json").read_text())
    _make_tree(tmp_path, listing)
    assert len(listing["cases"]) > 300
    for case in listing["cases"]:
        got = utils.get_all_files_in_path(case["src"], str(tmp_path), case["ignore"], case["dirs"])
        rel = [os.path.relpath(p, tmp_path) for p in got]
        assert rel == case["files"], case


def test_an_invalid_ignore_pattern_is_a_template_exception(tmp_path):
    (tmp_path / "a").write_text("x")
    with pytest.raises(TemplateException, match="pattern"):
        utils.get_all_files_in_path("a", str(tmp_path), ["[unterminated"], True)


def test_the_files_hash_follows_what_is_copied(tmp_path):
    (tmp_path / "d").mkdir()
    (tmp_path / "d" / "a.txt").write_text("one")
    (tmp_path / "d" / "skip.log").write_text("log")

    def h(ignore=(), src="d", dest="/x", resolve=False):
        return utils.calculate_files_hash(src, dest, str(tmp_path), list(ignore), resolve, None)

    first = h()
    assert first == h() and len(first) == 64
    assert h(dest="/y") != first  # the COPY text is part of the hash
    assert h(ignore=["*.log"]) == first  # patterns are anchored at the context root
    assert h(ignore=["**/*.log"]) != first  # an ignored file leaves the hash
    ignoring = h(ignore=["**/*.log"])
    (tmp_path / "d" / "skip.log").write_text("changed log")
    assert h(ignore=["**/*.log"]) == ignoring  # ... and its content no longer matters
    assert h() != first  # content
    (tmp_path / "d" / "a.txt").write_text("two")
    assert h(ignore=["**/*.log"]) != ignoring
    with pytest.raises(ValueError, match="No files found"):
        h(src="nothing/here")
    os.symlink("a.txt", tmp_path / "d" / "ln")
    assert h(resolve=True) != h(resolve=False)


def test_tar_file_stream_archives_the_selection(tmp_path):
    (tmp_path / "d" / "sub").mkdir(parents=True)
    (tmp_path / "d" / "a.txt").write_text("A")
    (tmp_path / "d" / "sub" / "b.txt").write_text("B")
    (tmp_path / "d" / "x.log").write_text("X")
    os.symlink("a.txt", tmp_path / "d" / "ln")
    for gz in (True, False):
        f = utils.tar_file_stream("d", str(tmp_path), ["**/*.log"], False, gz)
        data = f.read()
        f.close()
        assert (data[:2] == b"\x1f\x8b") is gz
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:*") as tar:
            names = sorted(tar.getnames())
            assert names == ["d", "d/a.txt", "d/ln", "d/sub", "d/sub/b.txt"]
            assert tar.getmember("d/ln").issym()
    f = utils.tar_file_stream("d", str(tmp_path), [], True, False)
    with tarfile.open(fileobj=f, mode="r:") as tar:
        assert tar.getmember("d/ln").isreg()  # followed
    f.close()


# ── the Dockerfile parser ──────────────────────────────────────────────────


class Recorder:
    """The builder methods a Dockerfile can call, recorded."""

    def __init__(self):
        self.calls = []

    def run_cmd(self, command, user=None):
        self.calls.append(["run_cmd", command, user])
        return self

    def copy(self, src, dest, force_upload=None, user=None, mode=None, **_):
        self.calls.append(["copy", src, dest, user, mode])
        return self

    def set_workdir(self, w):
        self.calls.append(["set_workdir", w])
        return self

    def set_user(self, u):
        self.calls.append(["set_user", u])
        return self

    def set_envs(self, e):
        self.calls.append(["set_envs", e])
        return self

    def set_start_cmd(self, c, r):
        self.calls.append(["set_start_cmd", c, r.get_cmd()])
        return self


def test_dockerfile_parser_makes_the_builder_calls_e2b_makes():
    fixture = json.loads((FIXTURES / "dockerfile_recorded.json").read_text())
    assert len(fixture["cases"]) >= 60
    for case in fixture["cases"]:
        rec, out = Recorder(), io.StringIO()
        try:
            with contextlib.redirect_stdout(out):
                got = {"base": parse_dockerfile(case["dockerfile"], rec), "calls": rec.calls}
        except Exception as e:  # noqa: BLE001
            got = {"error": type(e).__name__, "msg": str(e)}
        got["stdout"] = out.getvalue()
        assert got == case["expect"], case["dockerfile"]


def test_dockerfile_syntax_error_carries_the_line():
    with pytest.raises(DockerfileSyntaxError) as info:
        parse_dockerfile("FROM a\nBOGUS x", Recorder())
    assert info.value.line == 2 and "unknown instruction: BOGUS" in str(info.value)
    assert isinstance(info.value, ValueError)
    assert e2b.DockerfileSyntaxError is DockerfileSyntaxError


def test_dockerfile_can_be_a_path(tmp_path):
    p = tmp_path / "Dockerfile"
    p.write_text("FROM python:3\nRUN echo hi\n")
    rec = Recorder()
    assert parse_dockerfile(str(p), rec) == "python:3"
    assert ["run_cmd", "echo hi", None] in rec.calls


def test_from_dockerfile_builds_a_template_with_docker_defaults():
    t = Template().from_dockerfile(
        "FROM python:3.12\nENV A=1\nRUN pip install x\nCMD python app.py"
    )
    spec = json.loads(Template.to_json(t))
    assert spec["fromImage"] == "python:3.12"
    kinds = [s["type"] for s in spec["steps"]]
    assert kinds[:2] == ["USER", "WORKDIR"] and kinds[-2:] == ["USER", "WORKDIR"]
    assert spec["steps"][0]["args"] == ["root"] and spec["steps"][-1]["args"] == ["/home/user"]
    assert spec["startCmd"] == "python app.py" and spec["readyCmd"] == "sleep 20"


# ── the builder ────────────────────────────────────────────────────────────


def test_bases_set_the_image_or_the_template():
    t = Template()
    assert t._base_image == "e2bdev/base"
    expected = {
        "from_debian_image": ("debian:stable", ()),
        "from_ubuntu_image": ("ubuntu:24.04", ("24.04",)),
        "from_fedora_image": ("fedora:44", ()),
        "from_alpine_image": ("alpine:3.24", ()),
        "from_arch_image": ("archlinux:latest", ()),
        "from_python_image": ("python:3", ()),
        "from_node_image": ("node:lts", ()),
        "from_bun_image": ("oven/bun:latest", ()),
        "from_base_image": ("e2bdev/base", ()),
    }
    for method, (image, args) in expected.items():
        b = getattr(Template(), method)(*args)
        assert isinstance(b, TemplateBuilder) and b._template._base_image == image, method
    b = Template().from_template("base:v1")
    assert b._template._base_template == "base:v1" and b._template._base_image is None
    b2 = b._template.from_image("x:1")
    assert b2._template._base_template is None


def test_registry_credentials_are_recorded_and_validated(tmp_path):
    b = Template().from_image("reg.io/x:1", username="u", password="p")
    assert b._template._registry_config == {"type": "registry", "username": "u", "password": "p"}
    for kw in ({"username": "u"}, {"password": "p"}, {"username": "", "password": "p"}):
        with pytest.raises(InvalidArgumentException, match="Both username and password"):
            Template().from_image("reg.io/x:1", **kw)
    aws = Template().from_aws_registry("1.dkr.ecr.r.amazonaws.com/x", "AK", "SK", "eu-west-1")
    assert aws._template._registry_config == {
        "type": "aws",
        "awsAccessKeyId": "AK",
        "awsSecretAccessKey": "SK",
        "awsRegion": "eu-west-1",
    }
    (tmp_path / "sa.json").write_text('{"k": "v"}')
    gcp = Template(file_context_path=tmp_path).from_gcp_registry("gcr.io/p/x", "sa.json")
    assert gcp._template._registry_config == {"type": "gcp", "serviceAccountJson": '{"k": "v"}'}
    gcp2 = Template().from_gcp_registry("gcr.io/p/x", {"k": 1})
    assert json.loads(gcp2._template._registry_config["serviceAccountJson"]) == {"k": 1}
    spec = json.loads(Template.to_json(gcp))
    assert spec["fromImageRegistry"]["type"] == "gcp" and spec["fromImage"] == "gcr.io/p/x"


def test_instructions_have_e2bs_shapes():
    t = (
        Template()
        .from_base_image()
        .set_envs({"A": "1", "B": "two words"})
        .set_envs({})
        .set_workdir("/srv")
        .set_user("bob")
        .run_cmd(["a", "b"], user="root")
        .run_cmd("c")
        .remove(["/x", "/y z"], force=True, recursive=True)
        .rename("/a", "/b", force=True)
        .make_dir(["/d1", "/d2"], mode=0o755)
        .make_symlink("/t", "/l", force=True)
        .pip_install("numpy")
        .pip_install(["a", "b"], g=False)
        .pip_install()
        .npm_install(["x"], g=True, dev=True)
        .bun_install("y", dev=True)
        .apt_install("git", no_install_recommends=True, fix_missing=True)
        .git_clone("https://h/r.git", "/r", branch="main", depth=1, user="u")
    )
    steps = t._template._instructions
    by = [(s["type"].value, s["args"]) for s in steps]
    assert by[0] == ("ENV", ["A", "1", "B", "two words"])
    assert by[1] == ("WORKDIR", ["/srv"]) and by[2] == ("USER", ["bob"])
    assert by[3] == ("RUN", ["a && b", "root"]) and by[4] == ("RUN", ["c"])
    assert by[5] == ("RUN", ["rm -r -f /x '/y z'"])
    assert by[6] == ("RUN", ["mv /a /b -f"])
    assert by[7] == ("RUN", ["mkdir -p -m 0755 /d1 /d2"])
    assert by[8] == ("RUN", ["ln -s -f /t /l"])
    assert by[9] == ("RUN", ["pip install numpy", "root"])
    assert by[10] == ("RUN", ["pip install --user a b"])
    assert by[11] == ("RUN", ["pip install .", "root"])
    assert by[12] == ("RUN", ["npm install -g --save-dev x", "root"])
    assert by[13] == ("RUN", ["bun install --dev y"])
    assert by[14][1][1] == "root" and "--no-install-recommends --fix-missing git" in by[14][1][0]
    assert by[15] == (
        "RUN",
        ["git clone https://h/r.git --branch main --single-branch --depth 1 /r", "u"],
    )
    assert len(t._template._stack_traces) == len(steps) + 1  # base + one per instruction


def test_start_and_ready_commands_end_the_chain():
    t = base_template().run_cmd("x")
    final = t.set_start_cmd("serve", readycmd.wait_for_port(80))
    assert isinstance(final, TemplateFinal) and not hasattr(final, "run_cmd")
    assert t._template._start_cmd == "serve"
    assert t._template._ready_cmd == readycmd.wait_for_port(80).get_cmd()
    only_ready = base_template().set_ready_cmd("true")
    assert only_ready._template._ready_cmd == "true" and only_ready._template._start_cmd is None


def test_mcp_and_devcontainer_methods_need_their_base_templates():
    t = base_template()
    with pytest.raises(BuildException, match="mcp-gateway template"):
        t.add_mcp_server("exa")
    with pytest.raises(BuildException, match="devcontainer template"):
        t.beta_dev_container_prebuild("/d")
    with pytest.raises(BuildException, match="devcontainer template"):
        t.beta_set_dev_container_start("/d")
    g = Template().from_template("mcp-gateway")
    g.add_mcp_server(["brave", "exa"])
    assert g._template._instructions[-1]["args"] == ["mcp-gateway pull brave exa", "root"]
    d = Template().from_template("devcontainer")
    d.beta_dev_container_prebuild("/my dc")
    assert (
        d._template._instructions[-1]["args"][0] == "devcontainer build --workspace-folder '/my dc'"
    )
    final = d.beta_set_dev_container_start("/my dc")
    assert isinstance(final, TemplateFinal)
    assert "devcontainer up" in d._template._start_cmd
    assert d._template._ready_cmd == readycmd.wait_for_file("/devcontainer.up").get_cmd()


def test_skip_cache_forces_the_next_layers_and_the_whole_template():
    t = Template().from_base_image().run_cmd("a").skip_cache().run_cmd("b").set_workdir("/w")
    flags = [s["force"] for s in t._template._instructions]
    assert flags == [False, True, True]
    assert t._template._force is False
    t2 = Template().skip_cache().from_base_image()
    assert t2._template._force is True  # a FROM under skip_cache invalidates everything
    assert json.loads(Template.to_json(t2))["force"] is True


def test_copy_validates_and_records_the_files_hash(tmp_path):
    (tmp_path / "a.txt").write_text("a")
    (tmp_path / "b.txt").write_text("b")
    t = Template(file_context_path=tmp_path).from_base_image()
    with pytest.raises(TemplateException, match="absolute paths"):
        t.copy("/etc/passwd", "/x")
    with pytest.raises(TemplateException, match="escapes the context"):
        t.copy(["a.txt", "../x"], "/x")
    # sources are validated one by one, as in e2b: the valid one before the bad one is in
    assert [i["args"][0] for i in t._template._instructions] == ["a.txt"]
    del t._template._instructions[:]
    del t._template._stack_traces[1:]
    t.copy("a.txt", "/d/", user="bob:grp", mode=0o640, force_upload=True)
    t.copy_items([{"src": "a.txt", "dest": "/e", "gzip": False, "resolveSymlinks": True}])
    first, second = t._template._instructions
    assert first["args"] == ["a.txt", "/d/", "bob:grp", "0640"] and first["force"] is True
    assert second["gzip"] is False and second["resolveSymlinks"] is True
    t.copy(["a.txt", "b.txt"], "/two/")  # one instruction per source
    assert [i["args"][0] for i in t._template._instructions[2:]] == ["a.txt", "b.txt"]
    spec = json.loads(Template.to_json(t))
    assert all(len(s["filesHash"]) == 64 for s in spec["steps"])
    assert spec["steps"][0]["forceUpload"] is True and "forceUpload" not in spec["steps"][1]
    missing = Template(file_context_path=tmp_path).from_base_image().copy("missing", "/x")
    with pytest.raises(ValueError, match="No files found"):
        Template.to_json(missing)


def test_to_json_and_to_dockerfile_follow_e2b(tmp_path):
    (tmp_path / "f").write_text("f")
    t = (
        Template(file_context_path=tmp_path)
        .from_python_image("3.12")
        .set_envs({"A": "1", "B": "2"})
        .copy("f", "/f")
        .set_workdir("/w")
        .set_user("u")
        .run_cmd("echo hi")
        .set_start_cmd("run it", "true")
    )
    assert Template.to_dockerfile(t) == (
        "FROM python:3.12\nENV A=1 B=2\nCOPY f /f\nWORKDIR /w\nUSER u\nRUN echo hi\n"
        "ENTRYPOINT run it\n"
    )
    spec = json.loads(Template.to_json(t))
    assert spec["startCmd"] == "run it" and spec["readyCmd"] == "true" and spec["force"] is False
    assert [s["type"] for s in spec["steps"]] == ["ENV", "COPY", "WORKDIR", "USER", "RUN"]
    with pytest.raises(ValueError, match="another template"):
        Template.to_dockerfile(Template().from_template("x"))
    bare = TemplateBase.__new__(Template)
    bare.__init__()
    bare._base_image = None
    with pytest.raises(ValueError, match="No base image"):
        Template.to_dockerfile(TemplateBuilder(bare))


# ── building ───────────────────────────────────────────────────────────────


async def test_a_build_runs_the_flow_of_e2b_and_logs_it(backend, tmp_path):
    (tmp_path / "app").mkdir()
    (tmp_path / "app" / "main.py").write_text("print('hi')")
    logs = []
    t = (
        Template(file_context_path=tmp_path)
        .from_base_image()
        .set_envs({"MODE": "rl"})
        .copy("app", "/srv/app")
        .run_cmd("echo one")
        .set_start_cmd("serve", readycmd.wait_for_port(8080))
    )
    info = await AsyncTemplate.build(t, "web", on_build_logs=logs.append)
    assert isinstance(info, BuildInfo)
    assert (info.name, info.alias, info.template_id) == ("web", "web", "web")
    assert info.tags == ["default"] and len(info.build_id) == 16
    msgs = [e.message for e in logs]
    assert isinstance(logs[0], e2b.LogEntryStart) and isinstance(logs[-1], e2b.LogEntryEnd)
    order = [
        "Requesting build for template: web",
        f"Template created with ID: web, Build ID: {info.build_id}",
        "Uploaded 'app'",
        "All file uploads completed",
        "Starting building...",
        "Waiting for logs...",
        "step 1/3: ENV",
        "step 3/3: RUN",
        "start: serve",
        "template 'web' is ready",
    ]
    positions = [msgs.index(m) for m in order]
    assert positions == sorted(positions)
    machine = builder_of(backend, f"web-{info.build_id}")
    assert machine.deleted and machine.files["/srv/app/main.py"] == b"print('hi')"
    assert any("serve" in c for c in ran(machine))
    status = await AsyncTemplate.get_build_status(info)
    assert status.status == TemplateBuildStatus.READY and status.reason is None
    assert status.build_id == info.build_id and status.template_id == "web"


async def test_build_arguments_are_validated_like_e2bs():
    t = base_template()
    with pytest.raises(TemplateException, match="Name must be provided"):
        await AsyncTemplate.build(t)
    with pytest.raises(TemplateException, match="Name must be provided"):
        await AsyncTemplate.build_in_background(t, "")


async def test_a_build_by_alias_and_with_tags_in_the_name(backend):
    info = await AsyncTemplate.build(base_template().run_cmd("echo"), alias="by-alias")
    assert info.name == "by-alias" and await AsyncTemplate.exists("by-alias")
    tagged = await AsyncTemplate.build(base_template(), "svc:v1", tags=["stable", "v1", "x"])
    assert tagged.tags == ["v1", "stable", "x"]
    assert tagged.template_id == "svc" and tagged.name == "svc:v1"
    got = await AsyncTemplate.get_tags("svc")
    assert sorted(t.tag for t in got) == ["default", "stable", "v1", "x"]
    assert all(t.build_id == tagged.build_id for t in got)


async def test_tags_are_validated_when_a_build_is_requested(backend):
    for bad in (["a/b"], ["a:b"], [""]):
        with pytest.raises(BuildException, match="invalid tag"):
            await AsyncTemplate.build(base_template(), "t", tags=bad)
    assert snapshots(backend) == set()


async def test_cpu_memory_and_disk_are_validated(backend):
    for kw, text in (
        ({"cpu_count": 0}, "cpu_count must be at least 1"),
        ({"memory_mb": 0}, "memory_mb must be at least 1"),
        ({"cpu_count": -2}, "cpu_count must be a non-negative integer"),
        ({"min_free_disk_mb": -1}, "min_free_disk_mb must be a non-negative integer"),
        ({"memory_mb": "big"}, "memory_mb must be a non-negative integer"),
    ):
        with pytest.raises(BuildException, match=text):
            await AsyncTemplate.build(base_template(), "t", **kw)


async def test_cpu_and_memory_size_the_builder_vm_only_while_it_is_created(backend, monkeypatch):
    from e2b import _backend

    seen = []
    original = backend.create

    async def create(template, *, network=None, egress=None):
        seen.append(dict(_backend._SIZE.get() or {}))
        return await original(template, network=network, egress=egress)

    monkeypatch.setattr(backend, "create", create)
    await AsyncTemplate.build(base_template(), "sized", cpu_count=4, memory_mb=2048)
    await AsyncTemplate.build(base_template(), "plain")
    assert seen == [{"cpus": 4, "memory_mb": 2048}, {"cpus": None, "memory_mb": None}]
    assert _backend._SIZE.get() is None


async def test_min_free_disk_is_checked_after_the_steps(backend):
    ok = await AsyncTemplate.build(base_template(), "roomy", min_free_disk_mb=1000)
    assert ok.name == "roomy"
    backend.free_mb = 100
    with pytest.raises(
        BuildException, match="100 MiB free.*min_free_disk_mb=1000.*CUA_E2B_DISK_GB"
    ):
        await AsyncTemplate.build(base_template(), "tight", min_free_disk_mb=1000)
    assert not await AsyncTemplate.exists("tight")
    await AsyncTemplate.build(base_template(), "no-minimum", min_free_disk_mb=0)


async def test_skip_cache_and_force_upload_hold_because_nothing_is_cached(backend, tmp_path):
    (tmp_path / "f").write_text("f")
    t = Template(file_context_path=tmp_path).from_base_image().copy("f", "/f")
    for _ in range(2):  # a second identical build uploads again
        logs = []
        await AsyncTemplate.build(t, "again", skip_cache=True, on_build_logs=logs.append)
        assert any(e.message == "Uploaded 'f'" for e in logs)
        assert not any("Skipping upload" in e.message for e in logs)
    assert t._template._force is True
    t2 = Template(file_context_path=tmp_path).from_base_image().copy("f", "/f", force_upload=True)
    logs = []
    await AsyncTemplate.build(t2, "forced", on_build_logs=logs.append)
    assert any(e.message == "Uploaded 'f'" for e in logs)


@pytest.mark.parametrize(
    "image",
    ["python:3.12", "node:lts", "alpine:3.24", "debian:stable", "reg.io/team/app:1"],
)
async def test_a_bare_oci_base_is_refused_with_the_way_out(backend, image):
    t = Template().from_image(image).run_cmd("echo")
    with pytest.raises(BuildException, match="cua images build") as info:
        await AsyncTemplate.build(t, "oci")
    assert repr(image) in str(info.value)
    assert backend.template("oci") is None and backend.machines == {}


@pytest.mark.parametrize(
    "make, kind",
    [
        (lambda: Template().from_aws_registry("1.dkr.ecr.x/y", "AK", "SK", "r"), "aws"),
        (lambda: Template().from_gcp_registry("gcr.io/p/x", {"a": 1}), "gcp"),
        (lambda: Template().from_image("linux", username="u", password="p"), "registry"),
    ],
)
async def test_registry_credentials_are_refused_never_ignored(backend, make, kind):
    with pytest.raises(BuildException, match=f"registry credentials \\({kind}\\)"):
        await AsyncTemplate.build(make(), "creds")
    assert backend.machines == {}


@pytest.mark.parametrize(
    "image, cua",
    [
        ("e2bdev/base", "linux"),
        ("ubuntu:24.04", "linux"),
        ("linux", "linux"),
        ("macos", "macos"),
        ("ghcr.io/trycua/linux:24.04", "ghcr.io/trycua/linux:24.04"),
    ],
)
async def test_cua_images_are_built_on_as_vms(backend, image, cua):
    logs = []
    await AsyncTemplate.build(
        Template().from_image(image).run_cmd("echo"), "vm", on_build_logs=logs.append
    )
    assert next(iter(backend.machines.values())).template == cua
    assert any(e.message.startswith(f"base: {image}") for e in logs)


async def test_a_dockerfile_on_a_cua_base_builds(backend, tmp_path):
    (tmp_path / "x.txt").write_text("x")
    df = "FROM ubuntu:22.04\nENV A=1\nWORKDIR /w\nCOPY x.txt /w/\nRUN echo ok\nCMD echo up\n"
    t = Template(file_context_path=tmp_path).from_dockerfile(df)
    info = await AsyncTemplate.build(t, "from-df")
    machine = builder_of(backend, f"from-df-{info.build_id}")
    assert machine.files["/w/x.txt"] == b"x"
    commands = ran(machine)
    assert any("echo up" in c for c in commands)  # the CMD is the start command
    assert any("mkdir -p /w" in c for c in commands)


async def test_env_workdir_and_user_apply_to_the_following_runs(backend):
    t = (
        Template()
        .from_base_image()
        .set_envs({"K": "v"})
        .set_workdir("/srv")
        .set_user("root")
        .run_cmd("echo a")
        .set_user("bob")
        .run_cmd("echo b")
        .run_cmd("echo c", user="alice")
        .set_user("user")
        .run_cmd("echo d")
    )
    info = await AsyncTemplate.build(t, "ctx")
    machine = builder_of(backend, f"ctx-{info.build_id}")
    runs = {s["argv"][-1]: s for s in machine.spawned if s["argv"][-1].startswith("echo ")}
    assert runs["echo a"]["user"] == "root" and runs["echo a"]["cwd"] == "/srv"
    assert runs["echo a"]["env"]["K"] == "v"
    assert runs["echo b"]["user"] == "bob" and runs["echo c"]["user"] == "alice"
    assert runs["echo d"]["user"] in (None, "cua", "user")
    assert any("export K=v" in c for c in ran(machine))  # in every login shell of the template


async def _copy_machine(backend, tmp_path, build, name="cp"):
    info = await AsyncTemplate.build(build, name)
    return builder_of(backend, f"{name}-{info.build_id}")


async def test_copy_follows_dockers_placement_rules(backend, tmp_path):
    (tmp_path / "app" / "sub").mkdir(parents=True)
    (tmp_path / "app" / "main.py").write_text("M")
    (tmp_path / "app" / "sub" / "deep.txt").write_text("D")
    (tmp_path / "one.txt").write_text("1")
    (tmp_path / "two.txt").write_text("2")
    t = (
        Template(file_context_path=tmp_path)
        .from_base_image()
        .copy("app", "/srv/app")  # a directory: its contents go into dest
        .copy("one.txt", "/etc/renamed.txt")  # one file, no slash: dest is the file
        .copy("one.txt", "/into/dir/")  # a trailing slash: dest is a directory
        .copy("*.txt", "/many")  # several matches: dest is a directory
        .copy("app/*.py", "/glob/")
    )
    files = (await _copy_machine(backend, tmp_path, t)).files
    assert files["/srv/app/main.py"] == b"M" and files["/srv/app/sub/deep.txt"] == b"D"
    assert files["/etc/renamed.txt"] == b"1"
    assert files["/into/dir/one.txt"] == b"1"
    assert files["/many/one.txt"] == b"1" and files["/many/two.txt"] == b"2"
    assert files["/glob/main.py"] == b"M"


async def test_copy_applies_ignore_patterns_mode_owner_and_links(backend, tmp_path):
    (tmp_path / "d").mkdir()
    (tmp_path / "d" / "keep.txt").write_text("k")
    (tmp_path / "d" / "drop.log").write_text("l")
    (tmp_path / "d" / "ignored.tmp").write_text("t")
    (tmp_path / "d" / "run.sh").write_text("#!/bin/sh")
    os.chmod(tmp_path / "d" / "run.sh", 0o755)
    os.symlink("keep.txt", tmp_path / "d" / "ln")
    (tmp_path / ".dockerignore").write_text("**/*.log\n")
    t = (
        Template(file_context_path=tmp_path, file_ignore_patterns=["**/*.tmp"])
        .from_base_image()
        .copy("d", "/o", user="bob:grp")
        .copy("d/keep.txt", "/m/keep", mode=0o600)
    )
    machine = await _copy_machine(backend, tmp_path, t)
    assert set(machine.files) == {"/o/keep.txt", "/o/run.sh", "/m/keep"}
    commands = ran(machine)
    assert any(c.startswith("chmod 755 ") and "/o/run.sh" in c for c in commands)  # kept
    assert any(c.startswith("chmod 600 ") and "/m/keep" in c for c in commands)
    assert any("ln -sfn keep.txt /o/ln" in c for c in commands)
    assert machine.owners["/o"] == "bob" or any(c.startswith("chown bob:grp") for c in commands)
    assert any(c.startswith("chown bob:grp") for c in commands)


async def test_copy_can_follow_symlinks(backend, tmp_path):
    (tmp_path / "real.txt").write_text("R")
    os.symlink("real.txt", tmp_path / "ln")
    t = (
        Template(file_context_path=tmp_path)
        .from_base_image()
        .copy("ln", "/f", resolve_symlinks=True)
    )
    machine = await _copy_machine(backend, tmp_path, t)
    assert machine.files["/f"] == b"R"


async def test_the_chown_of_a_copy_defaults_to_the_current_user(backend, tmp_path):
    (tmp_path / "f").write_text("f")
    t = Template(file_context_path=tmp_path).from_base_image().set_user("bob").copy("f", "/x")
    machine = await _copy_machine(backend, tmp_path, t)
    assert any(c.startswith("chown bob ") for c in ran(machine))


async def test_a_failing_step_is_a_build_exception_with_its_step(backend):
    t = base_template().run_cmd("echo ok").run_cmd("fail 3")
    logs = []
    info = await AsyncTemplate.build_in_background(t, "bad", on_build_logs=logs.append)
    with pytest.raises(BuildException):
        await async_api.wait_for_build_finish(
            _engine.BuildClient(),
            info.template_id,
            info.build_id,
            logs.append,
            logs_refresh_frequency=0.01,
            stack_traces=t._template._stack_traces,
        )
    status = await AsyncTemplate.get_build_status(info)
    assert status.status == TemplateBuildStatus.ERROR
    assert status.reason.step == "2" and "exited with code 3" in status.reason.message
    assert status.reason.log_entries and status.reason.log_entries[-1].level == "error"
    assert backend.template("bad") is None and snapshots(backend) == set()


async def test_the_traceback_of_a_failed_build_points_at_the_failing_call(backend):
    t = base_template().run_cmd("echo ok").run_cmd("fail 3")  # the failing call is on this line
    line = t._template._stack_traces[2].tb_lineno
    with pytest.raises(BuildException) as info:
        await AsyncTemplate.build(t, "bad-tb")
    tb = info.value.__traceback__
    frames = []
    while tb:
        frames.append((tb.tb_frame.f_code.co_filename, tb.tb_lineno))
        tb = tb.tb_next
    assert (__file__, line) in frames


async def test_a_failed_rebuild_keeps_the_template_that_was_built(backend):
    first = await AsyncTemplate.build(base_template().run_cmd("echo v1"), "keep")
    with pytest.raises(BuildException):
        await AsyncTemplate.build(base_template().run_cmd("fail 1"), "keep")
    tags = await AsyncTemplate.get_tags("keep")
    assert [t.build_id for t in tags] == [first.build_id]
    assert snapshots(backend) == {f"keep-{first.build_id}"}


async def test_a_missing_start_dependency_fails_the_ready_check_step(backend):
    backend.permissive = True
    t = base_template().set_start_cmd("echo up", "fail 9")
    _engine.READY_TIMEOUT_S, saved = 0, _engine.READY_TIMEOUT_S
    try:
        with pytest.raises(BuildException, match="ready command never succeeded"):
            await AsyncTemplate.build(t, "never")
    finally:
        _engine.READY_TIMEOUT_S = saved


async def test_build_in_background_returns_at_once_and_is_followed_by_status(backend):
    t = base_template().run_cmd("echo a")
    info = await AsyncTemplate.build_in_background(t, "bg", tags=["t1"])
    assert info.name == "bg" and info.tags == ["t1"]
    first = await AsyncTemplate.get_build_status(info)
    assert first.status in (TemplateBuildStatus.BUILDING, TemplateBuildStatus.READY)
    for _ in range(200):
        status = await AsyncTemplate.get_build_status(info)
        if status.status == TemplateBuildStatus.READY:
            break
        await asyncio.sleep(0.01)
    assert status.status == TemplateBuildStatus.READY
    all_entries = status.log_entries
    assert any("step 1/1: RUN" in e.message for e in all_entries)
    rest = await AsyncTemplate.get_build_status(info, logs_offset=len(all_entries))
    assert rest.log_entries == []
    from_one = await AsyncTemplate.get_build_status(info, logs_offset=1)
    assert from_one.log_entries == all_entries[1:]
    unknown = BuildInfo("bg", "0" * 16, "bg", "bg")
    with pytest.raises(BuildException, match="404"):
        await AsyncTemplate.get_build_status(unknown)


async def test_a_build_status_returns_at_most_a_hundred_log_entries(backend):
    info = await AsyncTemplate.build(base_template(), "chatty")
    build = _engine._BUILDS[info.build_id]
    for i in range(250):
        build.log("info", f"line {i}")
    page = await AsyncTemplate.get_build_status(info)
    assert len(page.log_entries) == 100
    logs = []
    await async_api.wait_for_build_finish(
        _engine.BuildClient(), info.template_id, info.build_id, logs.append, 0.01
    )
    assert sum(1 for e in logs if e.message.startswith("line ")) == 250  # all of them, in pages


# ── the build_api functions ────────────────────────────────────────────────


async def test_the_build_api_functions_run_a_build_by_hand(backend, tmp_path):
    (tmp_path / "f.txt").write_text("F")
    t = (
        Template(file_context_path=tmp_path)
        .from_base_image()
        .copy("f.txt", "/f.txt")
        .run_cmd("echo")
    )
    client = _engine.BuildClient()
    response = await async_api.request_build(client, "manual:v2", ["extra"], None, None, None)
    assert response.template_id == "manual" and response.tags == ["v2", "extra"]
    steps = t._template._instructions_with_hashes()
    link = await async_api.get_file_upload_link(
        client, response.template_id, steps[0]["filesHash"], None
    )
    assert link.present is False and link.url
    await async_api.upload_file(client, "f.txt", str(tmp_path), link.url, [], False, True, None)
    await async_api.trigger_build(
        client, response.template_id, response.build_id, t._template._serialize(steps)
    )
    await async_api.wait_for_build_finish(
        client, response.template_id, response.build_id, None, 0.01, t._template._stack_traces
    )
    status = await async_api.get_build_status(client, response.template_id, response.build_id)
    assert status.status == TemplateBuildStatus.READY
    assert await async_api.check_alias_exists(client, "manual")
    assert await async_api.check_alias_exists(client, "manual:v2")
    assert not await async_api.check_alias_exists(client, "manual:nope")
    assert not await async_api.check_alias_exists(client, "ghost")
    machine = builder_of(backend, f"manual-{response.build_id}")
    assert machine.files["/f.txt"] == b"F"


async def test_a_copy_whose_files_were_not_uploaded_fails_the_build(backend, tmp_path):
    (tmp_path / "f.txt").write_text("F")
    t = Template(file_context_path=tmp_path).from_base_image().copy("f.txt", "/f.txt")
    client = _engine.BuildClient()
    response = await async_api.request_build(client, "no-upload", None, None, None, None)
    await async_api.trigger_build(
        client,
        response.template_id,
        response.build_id,
        t._template._serialize(t._template._instructions_with_hashes()),
    )
    with pytest.raises(BuildException, match="were not uploaded"):
        await async_api.wait_for_build_finish(
            client, response.template_id, response.build_id, None, 0.01
        )


async def test_the_build_api_refuses_what_it_cannot_serve(backend, tmp_path):
    client = _engine.BuildClient()
    with pytest.raises(e2b.FileUploadException, match="not a Cua build upload link"):
        await async_api.upload_file(
            client, "x", str(tmp_path), "https://s3/x", [], False, True, None
        )
    (tmp_path / "x").write_text("x")
    with pytest.raises(e2b.FileUploadException, match="Failed to upload file"):
        await async_api.upload_file(
            client, "x", str(tmp_path), "https://s3/x", [], False, True, None
        )
    with pytest.raises(BuildException, match="404"):
        await async_api.trigger_build(client, "t", "b" * 16, {"steps": [], "force": False})
    with pytest.raises(BuildException, match="404"):
        await async_api.get_build_status(client, "t", "b" * 16)


async def test_the_build_status_lists_logs_from_an_offset(backend):
    info = await AsyncTemplate.build(base_template().run_cmd("echo"), "offs")
    client = _engine.BuildClient()
    everything = await async_api.get_build_status(client, info.template_id, info.build_id)
    later = await async_api.get_build_status(client, info.template_id, info.build_id, 2)
    assert later.log_entries == everything.log_entries[2:]
    assert everything.logs == []


# ── tags ───────────────────────────────────────────────────────────────────


async def two_builds(backend):
    v1 = await AsyncTemplate.build(base_template().run_cmd("echo 1"), "app:v1")
    v2 = await AsyncTemplate.build(base_template().run_cmd("echo 2"), "app:v2")
    return v1, v2


async def test_every_tag_resolves_to_the_snapshot_of_its_build(backend):
    v1, v2 = await two_builds(backend)
    s1, s2 = f"app-{v1.build_id}", f"app-{v2.build_id}"
    assert backend.template("app:v1") == {
        "kind": "snapshot",
        "snapshot": s1,
        "template_id": "app",
        "tag": "v1",
        "build_id": v1.build_id,
    }
    assert backend.template("app:v2")["snapshot"] == s2
    # `app` and `app:default` are the latest build
    assert backend.template("app")["snapshot"] == s2
    assert backend.template("app:default")["snapshot"] == s2
    assert snapshots(backend) == {s1, s2}
    sb = await AsyncSandbox.create(template="app:v1")
    assert sb is not None  # the backend is asked for the template as the user named it


async def test_get_tags_lists_tags_with_their_builds(backend):
    v1, v2 = await two_builds(backend)
    tags = await AsyncTemplate.get_tags("app")
    assert all(isinstance(t, TemplateTag) and isinstance(t.created_at, datetime) for t in tags)
    assert {t.tag: t.build_id for t in tags} == {
        "v1": v1.build_id,
        "v2": v2.build_id,
        "default": v2.build_id,
    }
    assert [t.created_at for t in tags] == sorted(t.created_at for t in tags)
    assert {t.tag for t in await AsyncTemplate.get_tags("app:v1")} == {t.tag for t in tags}
    with pytest.raises(TemplateException, match="404") as info:
        await AsyncTemplate.get_tags("ghost")
    assert info.value.status_code == 404


async def test_assign_tags_points_tags_at_the_build_of_the_target(backend):
    v1, v2 = await two_builds(backend)
    one = await AsyncTemplate.assign_tags("app:v1", "production")
    assert isinstance(one, TemplateTagInfo)
    assert (one.build_id, one.tags) == (v1.build_id, ["production"])
    many = await AsyncTemplate.assign_tags("app:v2", ["stable", "canary", "stable"])
    assert (many.build_id, many.tags) == (v2.build_id, ["stable", "canary"])
    assert backend.template("app:production")["snapshot"] == f"app-{v1.build_id}"
    assert backend.template("app:stable")["snapshot"] == f"app-{v2.build_id}"
    moved = await AsyncTemplate.assign_tags("app:v2", "production")  # a tag moves
    assert moved.build_id == v2.build_id
    assert backend.template("app:production")["snapshot"] == f"app-{v2.build_id}"
    by_tag = {t.tag: t.build_id for t in await AsyncTemplate.get_tags("app")}
    assert by_tag["production"] == v2.build_id and by_tag["v1"] == v1.build_id
    # assigning `default` moves what plain `app` means
    await AsyncTemplate.assign_tags("app:v1", "default")
    assert backend.template("app")["snapshot"] == f"app-{v1.build_id}"
    with pytest.raises(TemplateException, match="404"):
        await AsyncTemplate.assign_tags("app:nope", "x")
    with pytest.raises(TemplateException, match="404"):
        await AsyncTemplate.assign_tags("ghost:v1", "x")
    with pytest.raises(TemplateException, match="at least one tag"):
        await AsyncTemplate.assign_tags("app:v1", [])
    with pytest.raises(TemplateException, match="invalid tag"):
        await AsyncTemplate.assign_tags("app:v1", "a/b")


async def test_a_moved_tag_deletes_the_snapshot_nothing_points_at_any_more(backend):
    v1, v2 = await two_builds(backend)
    await AsyncTemplate.assign_tags("app:v2", "v1")  # v1 now points at build 2
    assert snapshots(backend) == {f"app-{v2.build_id}"}
    assert backend.template("app:v1")["build_id"] == v2.build_id


async def test_remove_tags_unpoints_and_collects(backend):
    v1, v2 = await two_builds(backend)
    await AsyncTemplate.assign_tags("app:v1", ["old"])
    await AsyncTemplate.remove_tags("app", "v1")
    assert backend.template("app:v1") is None and backend.template("app:old") is not None
    assert snapshots(backend) == {f"app-{v1.build_id}", f"app-{v2.build_id}"}
    await AsyncTemplate.remove_tags("app", ["old"])
    assert backend.template("app:old") is None
    assert snapshots(backend) == {f"app-{v2.build_id}"}  # build 1 had no tag left
    assert not await AsyncTemplate.exists("app:v1") and await AsyncTemplate.exists("app:v2")
    with pytest.raises(TemplateException, match="default.*cannot be removed"):
        await AsyncTemplate.remove_tags("app", "default")
    with pytest.raises(TemplateException, match="404") as info:
        await AsyncTemplate.remove_tags("app", "never-there")
    assert info.value.status_code == 404
    with pytest.raises(TemplateException, match="404"):
        await AsyncTemplate.remove_tags("ghost", "x")
    assert {t.tag for t in await AsyncTemplate.get_tags("app")} == {"v2", "default"}


async def test_a_rebuild_moves_default_and_keeps_tagged_builds(backend):
    v1 = await AsyncTemplate.build(base_template(), "svc:v1")
    plain1 = await AsyncTemplate.build(base_template(), "svc")
    plain2 = await AsyncTemplate.build(base_template(), "svc")
    assert backend.template("svc")["snapshot"] == f"svc-{plain2.build_id}"
    assert snapshots(backend) == {f"svc-{v1.build_id}", f"svc-{plain2.build_id}"}
    assert plain1.build_id not in {t.build_id for t in await AsyncTemplate.get_tags("svc")}


async def test_exists_alias_exists_and_delete(backend):
    assert not await AsyncTemplate.exists("t") and not await AsyncTemplate.alias_exists("t")
    v1, v2 = await two_builds(backend)
    assert await AsyncTemplate.exists("app") and await AsyncTemplate.alias_exists("app")
    assert await AsyncTemplate.exists("app:v1") and not await AsyncTemplate.exists("app:v9")
    assert await AsyncTemplate.delete("app:v1") is True  # one tag
    assert await AsyncTemplate.delete("app:v1") is False
    assert await AsyncTemplate.exists("app") and not await AsyncTemplate.exists("app:v1")
    assert snapshots(backend) == {f"app-{v2.build_id}"}
    assert await AsyncTemplate.delete("app") is True  # the template, every tag and build
    assert snapshots(backend) == set() and backend.templates == {}
    assert await AsyncTemplate.delete("app") is False


async def test_a_template_saved_before_tags_existed_still_resolves(backend):
    await backend.save_template("old", {"kind": "snapshot", "snapshot": "old-snap"})
    assert await AsyncTemplate.exists("old")
    tags = await AsyncTemplate.get_tags("old")
    assert [t.tag for t in tags] == ["default"]
    await AsyncTemplate.assign_tags("old", "v1")
    assert backend.template("old:v1")["snapshot"] == "old-snap"


async def test_from_template_builds_on_a_tag(backend):
    base = await AsyncTemplate.build(base_template().run_cmd("echo base"), "base:v1")
    child = await AsyncTemplate.build(
        Template().from_template("base:v1").run_cmd("echo up"), "child"
    )
    machine = next(m for m in backend.machines.values() if m.template == "base:v1")
    assert machine.deleted
    assert f"base-{base.build_id}" in snapshots(backend) and child.name == "child"
    logs = []
    with pytest.raises(BuildException, match="could not start the builder VM"):
        backend.fail_create = RuntimeError("no such template")
        await AsyncTemplate.build(
            Template().from_template("ghost").run_cmd("x"), "orphan", on_build_logs=logs.append
        )
    assert any(e.message == "base: template ghost" for e in logs)


# ── the synchronous API goes through the sync build_api ────────────────────


def test_the_sync_template_runs_the_same_flow(backend, tmp_path, monkeypatch):
    (tmp_path / "f").write_text("f")
    calls = []
    for name in (
        "request_build",
        "get_file_upload_link",
        "upload_file",
        "trigger_build",
        "wait_for_build_finish",
    ):
        original = getattr(sync_api, name)

        def spy(*a, _n=name, _o=original, **k):
            calls.append(_n)
            return _o(*a, **k)

        monkeypatch.setattr("e2b.template_sync.main." + name, spy)
    t = Template(file_context_path=tmp_path).from_base_image().copy("f", "/f").run_cmd("echo")
    logs = []
    info = Template.build(t, "sync:v1", tags=["s"], on_build_logs=logs.append)
    assert calls == [
        "request_build",
        "get_file_upload_link",
        "upload_file",
        "trigger_build",
        "wait_for_build_finish",
    ]
    assert info.tags == ["v1", "s"] and isinstance(logs[0], e2b.LogEntryStart)
    assert Template.exists("sync") and Template.alias_exists("sync:s")
    status = Template.get_build_status(info)
    assert status.status == TemplateBuildStatus.READY
    assert {t.tag for t in Template.get_tags("sync")} == {"v1", "s", "default"}
    assigned = Template.assign_tags("sync:v1", ["a", "b"])
    assert assigned.tags == ["a", "b"] and assigned.build_id == info.build_id
    Template.remove_tags("sync", "a")
    Template.remove_tags("sync", ["b"])
    assert {t.tag for t in Template.get_tags("sync")} == {"v1", "s", "default"}
    assert Template.delete("sync:s") is True and Template.delete("sync") is True
    assert not Template.exists("sync")


def test_the_sync_build_api_functions(backend, tmp_path):
    (tmp_path / "f.txt").write_text("F")
    t = Template(file_context_path=tmp_path).from_base_image().copy("f.txt", "/f.txt")
    client = _engine.BuildClient()
    response = sync_api.request_build(client, "syncapi", None, 2, 1024, 0)
    steps = t._template._instructions_with_hashes()
    link = sync_api.get_file_upload_link(client, "syncapi", steps[0]["filesHash"])
    sync_api.upload_file(client, "f.txt", str(tmp_path), link.url, [], False, True, None)
    sync_api.trigger_build(client, "syncapi", response.build_id, t._template._serialize(steps))
    seen = []
    sync_api.wait_for_build_finish(client, "syncapi", response.build_id, seen.append, 0.01)
    assert any(e.message == "template 'syncapi' is ready" for e in seen)
    assert sync_api.check_alias_exists(client, "syncapi")
    assert (
        sync_api.get_build_status(client, "syncapi", response.build_id, 1).log_entries == seen[1:]
    )
    assert [t.tag for t in sync_api.get_template_tags(client, "syncapi")] == ["default"]
    assert sync_api.assign_tags(client, "syncapi", ["x"]).tags == ["x"]
    sync_api.remove_tags(client, "syncapi", ["x"])


def test_a_failing_sync_build_raises_build_exception(backend):
    t = Template().from_base_image().run_cmd("fail 4")
    with pytest.raises(BuildException, match="exited with code 4"):
        Template.build(t, "sync-bad")
    with pytest.raises(TemplateException, match="Name must be provided"):
        Template.build(t)


def test_the_sync_build_in_background(backend):
    info = Template.build_in_background(Template().from_base_image().run_cmd("echo"), "sync-bg")
    for _ in range(300):
        if Template.get_build_status(info).status == TemplateBuildStatus.READY:
            break
        import time

        time.sleep(0.01)
    assert Template.get_build_status(info).status == TemplateBuildStatus.READY


def test_connection_options_resolve_through_the_client_factory(backend):
    class Bound(Template):
        _bound_api_params = {"domain": "bound.example"}

    seen = []
    original = _engine.BuildClient

    class Spy(original):
        def __init__(self, api_params=None, **k):
            seen.append(api_params)
            super().__init__(api_params=api_params, **k)

    import e2b.template_sync.main as sync_main

    sync_main.BuildClient = Spy
    try:
        Bound.exists("whatever")
        Bound.exists("whatever", domain="call.example")
    finally:
        sync_main.BuildClient = original
    assert seen[0]["domain"] == "bound.example" and seen[1]["domain"] == "call.example"
