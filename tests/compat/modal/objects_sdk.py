"""The unmodified `modal` client against the gateway: Volume, Dict, Queue,
Secret, App, Environment, Client, Image builders and the Sandbox calls the
other suites do not make. Hermetic (mock Spaces); the calls that need a real
guest (directory snapshots, bind mounts, memory snapshots) are in
conformance.py, which runs on real Spaces.

Each check asserts SDK-visible behaviour; any failure exits 1.
"""

from __future__ import annotations

import io
import json
import os
import queue
import sys
import tempfile
import time
import traceback
import uuid

# With an env file (the hermetic runners) its values are the gateway's; without,
# the environment is (`eval "$(cua compat env --provider modal)"`).
if len(sys.argv) > 1:
    os.environ.update(json.loads(open(sys.argv[1]).read()))

import modal  # noqa: E402
import modal.billing  # noqa: E402
import modal.experimental  # noqa: E402
from modal.exception import (  # noqa: E402
    AlreadyExistsError,
    InvalidError,
    NotFoundError,
    UnimplementedError,
)

failures: list[str] = []
passed = 0
RUN = uuid.uuid4().hex[:8]


def check(name, fn):
    global passed
    try:
        fn()
        passed += 1
        print(f"PASS {name}")
    except BaseException as e:  # noqa: BLE001
        failures.append(name)
        print(f"FAIL {name}: {type(e).__name__}: {e}")
        traceback.print_exc()


def raises(exc, fn):
    try:
        fn()
    except exc as e:
        return e
    raise AssertionError(f"expected {exc.__name__}")


def eq(a, b):
    assert a == b, f"{a!r} != {b!r}"


def n(prefix):
    return f"{prefix}-{RUN}"


# ---- Volume ------------------------------------------------------------------


def volume_lifecycle():
    name = n("vol")
    raises(NotFoundError, lambda: modal.Volume.from_name(name).hydrate())
    v = modal.Volume.from_name(name, create_if_missing=True)
    v.hydrate()
    assert v.object_id.startswith("vo-"), v.object_id
    eq(v.name, name)
    again = modal.Volume.from_name(name)
    again.hydrate()
    eq(again.object_id, v.object_id)
    info = v.info()
    eq(info.name, name)
    assert info.created_at is not None
    modal.Volume.objects.create(n("vol2"))
    raises(AlreadyExistsError, lambda: modal.Volume.objects.create(n("vol2")))
    modal.Volume.objects.create(n("vol2"), allow_existing=True)
    names = [x.name for x in modal.Volume.objects.list()]
    assert name in names and n("vol2") in names, names
    modal.Volume.rename(n("vol2"), n("vol3"))
    names = [x.name for x in modal.Volume.objects.list()]
    assert n("vol3") in names and n("vol2") not in names, names
    modal.Volume.objects.delete(n("vol3"))
    raises(NotFoundError, lambda: modal.Volume.objects.delete(n("vol3")))
    modal.Volume.objects.delete(n("vol3"), allow_missing=True)


def volume_ephemeral():
    with modal.Volume.ephemeral() as v:
        v.hydrate()
        assert v.object_id.startswith("vo-")
        with v.batch_upload() as b:
            b.put_file(io.BytesIO(b"x"), "/a")
        eq(b"".join(v.read_file("a")), b"x")


def volume_files():
    v = modal.Volume.from_name(n("files"), create_if_missing=True)
    big = os.urandom(3 * 1024 * 1024 + 17)
    with tempfile.TemporaryDirectory() as d:
        open(os.path.join(d, "l.txt"), "w").write("local")
        os.makedirs(os.path.join(d, "sub"))
        open(os.path.join(d, "sub", "m.txt"), "w").write("deep")
        with v.batch_upload() as b:
            b.put_file(io.BytesIO(b"hello"), "/dir/hello.txt")
            b.put_file(io.BytesIO(big), "/dir/big.bin")
            b.put_file(os.path.join(d, "l.txt"), "/from_local.txt")
            b.put_directory(os.path.join(d, "sub"), "/tree")
    eq(b"".join(v.read_file("dir/hello.txt")), b"hello")
    eq(b"".join(v.read_file("/dir/big.bin")), big)
    out = io.BytesIO()
    eq(v.read_file_into_fileobj("dir/big.bin", out), len(big))
    eq(out.getvalue(), big)
    top = {e.path: e for e in v.listdir("/")}
    assert {"dir", "from_local.txt", "tree"} <= set(top), top
    eq(top["dir"].type, modal.volume.FileEntryType.DIRECTORY)
    eq(top["from_local.txt"].type, modal.volume.FileEntryType.FILE)
    eq(top["from_local.txt"].size, 5)
    rec = {e.path for e in v.listdir("/", recursive=True)}
    assert {"dir/hello.txt", "dir/big.bin", "tree/m.txt"} <= rec, rec
    eq({e.path for e in v.iterdir("dir", recursive=False)}, {"dir/hello.txt", "dir/big.bin"})
    eq(len(list(v.iterdir("/"))), len(rec))
    v.copy_files(["dir/hello.txt"], "copy.txt")
    eq(b"".join(v.read_file("copy.txt")), b"hello")
    # Default (version 1) volumes: the client refuses a recursive copy itself.
    raises(ValueError, lambda: v.copy_files(["dir"], "moved", recursive=True))
    raises(
        InvalidError, lambda: v.copy_files(["dir/hello.txt", "tree/m.txt"], "moved")
    )  # cp: not a directory
    v.copy_files(["tree/m.txt", "from_local.txt"], "dir")
    eq(b"".join(v.read_file("dir/m.txt")), b"deep")
    v.remove_file("dir/m.txt")
    v.remove_file("dir/from_local.txt")
    v.remove_file("copy.txt")
    raises(FileNotFoundError, lambda: list(v.read_file("copy.txt")))
    raises(Exception, lambda: v.remove_file("tree"))  # a directory needs recursive
    v.remove_file("tree", recursive=True)
    raises(NotFoundError, lambda: v.listdir("tree"))
    v.commit()
    v.reload()

    # An existing file is refused unless forced.
    def overwrite(force):
        with v.batch_upload(force=force) as b:
            b.put_file(io.BytesIO(b"new"), "/dir/hello.txt")

    raises(FileExistsError, lambda: overwrite(False))
    overwrite(True)
    eq(b"".join(v.read_file("dir/hello.txt")), b"new")


def volume_views():
    v = modal.Volume.from_name(n("files"), create_if_missing=True)
    ro = v.read_only()
    v2 = v.with_mount_options(sub_path="dir")
    assert ro is not v and v2 is not v


# ---- Dict ----------------------------------------------------------------------


def dict_ops():
    name = n("d")
    raises(NotFoundError, lambda: modal.Dict.from_name(name).hydrate())
    d = modal.Dict.from_name(name, create_if_missing=True)
    d.hydrate()
    assert d.object_id.startswith("di-"), d.object_id
    eq(d.name, name)
    eq(d.len(), 0)
    d.put("a", 1)
    d["b"] = {"x": [1, 2]}
    eq(d.get("a"), 1)
    eq(d["b"], {"x": [1, 2]})
    eq(d.get("zzz", "dflt"), "dflt")
    raises(KeyError, lambda: d["zzz"])
    eq(d.put("a", 2, skip_if_exists=True), False)
    eq(d.get("a"), 1)
    eq(d.put("c", 3, skip_if_exists=True), True)
    assert d.contains("a") and "a" in d and not d.contains("q")
    d.update({"e": 5}, f=6)
    eq(sorted(d.keys()), ["a", "b", "c", "e", "f"])
    eq(dict(d.items())["f"], 6)
    eq(sorted(v for v in d.values() if isinstance(v, int)), [1, 3, 5, 6])
    eq(d.len(), 5)
    eq(d.pop("a"), 1)
    raises(KeyError, lambda: d.pop("a"))
    eq(d.pop("a", "gone"), "gone")
    del d["b"]
    d.clear()
    eq(d.len(), 0)
    eq(d.info().name, name)
    assert name in [x.name for x in modal.Dict.objects.list()]
    modal.Dict.objects.create(n("d2"))
    raises(AlreadyExistsError, lambda: modal.Dict.objects.create(n("d2")))
    modal.Dict.objects.delete(n("d2"))
    raises(NotFoundError, lambda: modal.Dict.objects.delete(n("d2")))
    modal.Dict.objects.delete(name)


def dict_ephemeral_and_from_id():
    with modal.Dict.ephemeral() as d:
        d.put("k", "v")
        eq(d.get("k"), "v")
        same = modal.Dict.from_id(d.object_id)
        eq(same.get("k"), "v")


def dict_values_are_pickled_python():
    d = modal.Dict.from_name(n("pickle"), create_if_missing=True)
    d.put((1, "t", None), {"set": {1, 2}, "bytes": b"\x00\xff"})
    eq(d.get((1, "t", None)), {"set": {1, 2}, "bytes": b"\x00\xff"})


# ---- Queue ---------------------------------------------------------------------


def queue_ops():
    name = n("q")
    raises(NotFoundError, lambda: modal.Queue.from_name(name).hydrate())
    q = modal.Queue.from_name(name, create_if_missing=True)
    q.hydrate()
    assert q.object_id.startswith("qu-"), q.object_id
    eq(q.name, name)
    eq(q.len(), 0)
    q.put(1)
    q.put_many([2, 3, 4])
    eq(q.len(), 4)
    eq(q.get(), 1)
    eq(q.get_many(2), [2, 3])
    eq(q.get(block=False), 4)
    eq(q.get(block=False), None)
    t0 = time.time()
    raises(queue.Empty, lambda: q.get(timeout=0.4))
    assert time.time() - t0 >= 0.3
    raises(queue.Empty, lambda: q.get_many(2, timeout=0.2))
    q.put_many(["a", "b"])
    eq(list(q.iterate()), ["a", "b"])
    q.put_many(["x", "y", "z"])
    q.clear()
    eq(q.len(), 0)
    eq(q.info().name, name)
    assert name in [x.name for x in modal.Queue.objects.list()]
    modal.Queue.objects.create(n("q2"))
    raises(AlreadyExistsError, lambda: modal.Queue.objects.create(n("q2")))
    modal.Queue.objects.delete(n("q2"))
    modal.Queue.objects.delete(name)


def queue_partitions():
    q = modal.Queue.from_name(n("qp"), create_if_missing=True)
    q.put("default", partition=None)
    q.put("p1", partition="one")
    q.put_many(["p2a", "p2b"], partition="two")
    eq(q.len(), 1)
    eq(q.len(partition="two"), 2)
    eq(q.len(total=True), 4)
    eq(q.get(partition="one"), "p1")
    eq(q.get_many(5, block=False, partition="two"), ["p2a", "p2b"])
    q.clear(partition="one")
    q.clear(all=True)
    eq(q.len(total=True), 0)
    raises(InvalidError, lambda: q.put(1, partition=""))
    eq(modal.Queue.validate_partition_key("a"), b"a")
    raises(InvalidError, lambda: modal.Queue.validate_partition_key("x" * 65))


def queue_blocking_get_wakes_on_put():
    import threading

    q = modal.Queue.from_name(n("qb"), create_if_missing=True)
    threading.Timer(0.5, lambda: q.put("late")).start()
    t0 = time.time()
    eq(q.get(timeout=10), "late")
    assert time.time() - t0 < 5


def queue_ephemeral_and_from_id():
    with modal.Queue.ephemeral() as q:
        q.put(7)
        eq(modal.Queue.from_id(q.object_id).get(), 7)


# ---- Secret ----------------------------------------------------------------------


def secret_named_objects():
    name = n("sec")
    raises(NotFoundError, lambda: modal.Secret.from_name(name).hydrate())
    modal.Secret.objects.create(name, {"A": "1", "B": "2"})
    raises(AlreadyExistsError, lambda: modal.Secret.objects.create(name, {"A": "x"}))
    modal.Secret.objects.create(name, {"A": "9", "B": "2"}, allow_existing=True)
    s = modal.Secret.from_name(name)
    s.hydrate()
    assert s.object_id.startswith("st-"), s.object_id
    eq(s.info().name, name)
    raises(NotFoundError, lambda: modal.Secret.from_name(name, required_keys=["MISSING"]).hydrate())
    modal.Secret.from_name(name, required_keys=["A"]).hydrate()
    s.update({"C": "3"})
    assert name in [x.name for x in modal.Secret.objects.list()]
    modal.Secret.objects.delete(name)
    raises(NotFoundError, lambda: modal.Secret.objects.delete(name))
    modal.Secret.objects.delete(name, allow_missing=True)


def secret_builders():
    s = modal.Secret.from_dict({"X": "1", "NONE": None})
    s.hydrate()
    assert s.object_id.startswith("st-")
    os.environ["CUA_TEST_SECRET_ENV"] = "from-environ"
    modal.Secret.from_local_environ(["CUA_TEST_SECRET_ENV"]).hydrate()
    raises(
        InvalidError, lambda: modal.Secret.from_local_environ(["CUA_NOT_SET_ANYWHERE"]).hydrate()
    )
    with tempfile.TemporaryDirectory() as d:
        open(os.path.join(d, ".env"), "w").write("K1=v1\nK2='v 2'\n# c\n")
        modal.Secret.from_dotenv(d).hydrate()
        modal.Secret.from_dotenv(os.path.join(d, ".env")).hydrate()


# ---- App, Environment, Client ------------------------------------------------------


def app_lookup_tags_info():
    name = n("app")
    raises(NotFoundError, lambda: modal.App.lookup(name))
    app = modal.App.lookup(name, create_if_missing=True)
    assert app.app_id.startswith("ap-"), app.app_id
    eq(app.name, name)
    again = modal.App.lookup(name)
    eq(again.app_id, app.app_id)
    app.set_tags({"team": "x", "k": "v"})
    eq(app.get_tags(), {"team": "x", "k": "v"})
    info = app.info(refresh=True)
    eq(info.app_id, app.app_id)
    assert app.get_dashboard_url().startswith("http")


def app_logs_stream():
    app = modal.App.lookup(n("applogs"), create_if_missing=True)
    sb = modal.Sandbox.create("echo", "log-line", app=app, image=IMAGE, timeout=60)
    try:
        sb.wait()
        import datetime

        since = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=5)
        fetched = "".join(e.message for e in app.logs.fetch(since=since))
        assert "log-line" in fetched, fetched
        tailed = "".join(e.message for e in app.logs.tail(10))
        assert "log-line" in tailed, tailed
        streamed = ""
        for e in app.logs.stream(timeout=5):
            streamed += e.message
            if "log-line" in streamed:
                break
        assert "log-line" in streamed, streamed
    finally:
        sb.terminate()


def environments():
    assert any(e.name == "main" for e in modal.Environment.objects.list())
    modal.Environment.objects.create(n("env"))
    raises(AlreadyExistsError, lambda: modal.Environment.objects.create(n("env")))
    e = modal.Environment.from_name(n("env"))
    e.hydrate()
    eq(e.name, n("env"))
    modal.Environment.objects.delete(n("env"))
    raises(NotFoundError, lambda: modal.Environment.from_name(n("env")).hydrate())


def client_calls():
    c = modal.Client.from_env()
    c.hello()
    assert not c.is_closed()
    modal.Client.verify(
        os.environ["MODAL_SERVER_URL"],
        (os.environ["MODAL_TOKEN_ID"], os.environ["MODAL_TOKEN_SECRET"]),
    )
    c2 = modal.Client.from_credentials(
        os.environ["MODAL_TOKEN_ID"], os.environ["MODAL_TOKEN_SECRET"]
    )
    c2.hello()
    bad = modal.Client.from_credentials("ak-bad", "as-bad")
    raises(Exception, bad.hello)
    ws = modal.Workspace.from_context()
    ws.hydrate()
    assert ws.name


# ---- Image builders ------------------------------------------------------------------


IMAGE = modal.Image.from_registry("ghcr.io/trycua/linux:24.04")


def build(img):
    app = modal.App.lookup(n("imgapp"), create_if_missing=True)
    img.build(app)
    assert img.object_id.startswith("im-"), img.object_id
    return img


def image_builders():
    cases = {
        "debian_slim": modal.Image.debian_slim(python_version="3.12"),
        "run_commands": IMAGE.run_commands("echo a", "echo b"),
        "apt_install": IMAGE.apt_install("curl", "git"),
        "pip_install": IMAGE.pip_install("requests==2.32.0", "rich"),
        "uv_pip_install": IMAGE.uv_pip_install("rich"),
        "env_workdir_cmd_entrypoint": IMAGE.env({"A": "1"})
        .workdir("/work")
        .cmd(["echo", "hi"])
        .entrypoint([]),
        "shell": IMAGE.shell(["/bin/bash", "-c"]),
        "dockerfile_commands": IMAGE.dockerfile_commands(["RUN echo hi", "ENV Z=1"]),
        "micromamba": modal.Image.micromamba(python_version="3.11").micromamba_install("numpy"),
        "from_scratch": modal.Image.from_scratch(),
        "add_python": modal.Image.from_registry("ubuntu:22.04", add_python="3.11"),
        "add_local_python_source": modal.Image.debian_slim().add_local_python_source(
            "json", copy=True
        ),
        "pip_install_private_repos": modal.Image.debian_slim().pip_install_private_repos(
            "github.com/x/y", git_user="u", secrets=[modal.Secret.from_dict({"GITHUB_TOKEN": "t"})]
        ),
    }
    for name, img in cases.items():
        try:
            build(img)
        except Exception as e:  # noqa: BLE001
            raise AssertionError(f"{name}: {type(e).__name__}: {e}") from e
    ids = {name: img.object_id for name, img in cases.items()}
    eq(len(set(ids.values())), len(ids))


def image_dockerfile_and_requirements():
    with tempfile.TemporaryDirectory() as d:
        open(os.path.join(d, "Dockerfile"), "w").write(
            "FROM ghcr.io/trycua/linux:24.04\nRUN echo from-file\n"
        )
        open(os.path.join(d, "requirements.txt"), "w").write("rich\n")
        open(os.path.join(d, "pyproject.toml"), "w").write(
            '[project]\nname="x"\nversion="0"\ndependencies=["rich"]\n[project.optional-dependencies]\ndev=["pytest"]\n'
        )
        build(modal.Image.from_dockerfile(os.path.join(d, "Dockerfile")))
        build(IMAGE.pip_install_from_requirements(os.path.join(d, "requirements.txt")))
        build(
            IMAGE.pip_install_from_pyproject(
                os.path.join(d, "pyproject.toml"), optional_dependencies=["dev"]
            )
        )
        os.makedirs(os.path.join(d, "pkg"))
        open(os.path.join(d, "pkg", "__init__.py"), "w").write("x = 1\n")
        build(IMAGE.add_local_dir(d, "/srv/d", ignore=["*.txt"]))
        build(IMAGE.add_local_file(os.path.join(d, "requirements.txt"), "/srv/r.txt", copy=True))


def image_from_id_and_name_and_publish():
    img = build(IMAGE.run_commands("echo for-id"))
    img.publish(n("published"))
    named = modal.Image.from_name(n("published"))
    app2 = modal.App.lookup(n("imgapp"), create_if_missing=True)
    sbn = modal.Sandbox.create("sleep", "60000", app=app2, image=named, timeout=60)
    sbn.terminate()
    named_missing = modal.Image.from_name(n("never-published"))
    raises(
        NotFoundError,
        lambda: modal.Sandbox.create("sleep", "1", app=app2, image=named_missing, timeout=60),
    )
    # An Image by id is used where an image goes (it cannot be hydrated alone).
    same = modal.Image.from_id(img.object_id)
    app = modal.App.lookup(n("imgapp"), create_if_missing=True)
    sb = modal.Sandbox.create("sleep", "60000", app=app, image=same, timeout=60)
    try:
        assert sb.object_id
    finally:
        sb.terminate()


def image_logs_and_more_builders():
    import datetime

    img = build(IMAGE.run_commands(f"echo build-log-{RUN}"))
    text = "".join(e.message for e in img.logs.fetch(layers=None))
    assert f"build-log-{RUN}" in text, text
    assert "build-log" in "".join(e.message for e in img.logs.tail(50))
    # A second look at the same (cached) layer has the same log.
    again = build(IMAGE.run_commands(f"echo build-log-{RUN}"))
    eq(again.object_id, img.object_id)
    assert f"build-log-{RUN}" in "".join(e.message for e in again.logs.fetch(layers=None))
    with tempfile.TemporaryDirectory() as d:
        open(os.path.join(d, "pyproject.toml"), "w").write(
            '[project]\nname="x"\nversion="0"\nrequires-python=">=3.10"\ndependencies=[]\n'
            '[tool.poetry]\nname="x"\n'
        )
        open(os.path.join(d, "poetry.lock"), "w").write("# lock\n")
        build(IMAGE.poetry_install_from_file(os.path.join(d, "pyproject.toml")))
        build(IMAGE.pip_install_from_pyproject(os.path.join(d, "pyproject.toml")))
    # Client-side helpers.
    with IMAGE.imports():
        pass


def _noop():
    pass


def image_unsupported():
    # A private registry needs credentials this gateway does not hold.
    for make in (
        lambda: modal.Image.from_aws_ecr(
            "123456789012.dkr.ecr.us-east-1.amazonaws.com/x:1", modal.Secret.from_dict({"A": "b"})
        ),
        lambda: modal.Image.from_gcp_artifact_registry(
            "us-docker.pkg.dev/p/r/i:1", modal.Secret.from_dict({"A": "b"})
        ),
    ):
        e = raises(Exception, lambda: build(make()))
        assert isinstance(e, (UnimplementedError, modal.exception.ImageBuildError, InvalidError)), e


def file_pattern_matcher():
    m = modal.FilePatternMatcher("*.py", "!keep.py")
    assert m(__import__("pathlib").Path("a.py")) and not m(__import__("pathlib").Path("keep.py"))
    assert (~m)(__import__("pathlib").Path("a.txt"))
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "ignore")
        open(p, "w").write("*.log\n")
        assert modal.FilePatternMatcher.from_file(p)(__import__("pathlib").Path("x.log"))


# ---- Errors the gateway answers with Modal's own types ------------------------------------


def refused_products():
    for what, fn in {
        "proxy": lambda: modal.Proxy.from_name("p").hydrate(),
        "nfs": lambda: modal.NetworkFileSystem.from_name("n", create_if_missing=True).hydrate(),
        "cluster": lambda: modal.Cluster.from_id("cl-xxx").hydrate(),
        "server": lambda: modal.Server.from_name("app", "srv").hydrate(),
    }.items():
        e = raises(Exception, fn)
        assert isinstance(
            e, (UnimplementedError, NotFoundError, InvalidError, modal.exception.Error)
        ), (what, e)


def refused(fn, what):
    e = raises(Exception, fn)
    assert isinstance(e, UnimplementedError) and "Cua Spaces" in str(e), (
        what,
        type(e).__name__,
        str(e)[:200],
    )


def workspace_and_environment_administration_is_refused():
    import datetime

    ws = modal.Workspace.from_context()
    ws.hydrate()
    refused(lambda: ws.members.list(), "Workspace.members")
    refused(lambda: ws.service_users.list(), "Workspace.service_users")
    refused(lambda: ws.proxy_tokens.list(), "Workspace.proxy_tokens")
    refused(lambda: ws.settings.list(), "Workspace.settings")
    refused(lambda: ws.billing.summary(), "Workspace.billing")
    env = modal.Environment.from_name("main")
    env.hydrate()
    refused(lambda: env.members.list(), "Environment.members")
    refused(lambda: env.roles.list(), "Environment.roles")
    refused(lambda: env.billing.summary(), "Environment.billing")
    refused(
        lambda: modal.billing.workspace_billing_report(
            start=datetime.datetime.now() - datetime.timedelta(days=1)
        ),
        "billing.workspace_billing_report",
    )
    # The Apps of an Environment are served.
    assert isinstance(env.apps.list(), list)


def network_products_are_refused():
    refused(
        lambda: modal.NetworkFileSystem.from_name("n", create_if_missing=True).hydrate(),
        "NetworkFileSystem",
    )
    refused(lambda: modal.Cluster.from_id("cl-x").hydrate(), "Cluster")
    refused(lambda: modal.experimental.list_cluster_reservations(), "list_cluster_reservations")
    e = raises(Exception, lambda: modal.forward(8000).__enter__())
    assert "container" in str(e), e  # client-side: only valid inside a Function container
    e = raises(Exception, lambda: modal.interact())
    assert "container" in str(e), e


def experimental_app_calls():
    app = modal.App("exp-named")
    app.function(serialized=True, name="f")(_noop)
    app.deploy(name=f"exp-named-{RUN}")
    names = [a.name for a in modal.experimental.list_deployed_apps()]
    assert f"exp-named-{RUN}" in names, names
    assert modal.experimental.get_app_lifecycle(app.app_id).deployed_at is not None
    assert "f" in modal.experimental.get_app_objects(f"exp-named-{RUN}")
    modal.experimental.stop_app(f"exp-named-{RUN}")
    assert f"exp-named-{RUN}" not in [a.name for a in modal.experimental.list_deployed_apps()]
    img = build(IMAGE.run_commands("echo exp-delete"))
    sb = modal.Sandbox.create(
        "sleep",
        "60000",
        app=modal.App.lookup(n("imgapp"), create_if_missing=True),
        image=img,
        timeout=60,
    )
    try:
        raises(
            modal.exception.ConflictError, lambda: modal.experimental.image_delete(img.object_id)
        )  # in use
    finally:
        sb.terminate()
    modal.experimental.image_delete(img.object_id)
    raises(NotFoundError, lambda: modal.experimental.image_delete(img.object_id))


def main():
    for name, fn in list(globals().items()):
        if callable(fn) and getattr(fn, "__module__", None) == "__main__" and name in CHECKS:
            check(name, fn)
    print(f"{passed} passed, {len(failures)} failed: {failures}")
    sys.exit(1 if failures else 0)


CHECKS = {
    "volume_lifecycle",
    "volume_ephemeral",
    "volume_files",
    "volume_views",
    "dict_ops",
    "dict_ephemeral_and_from_id",
    "dict_values_are_pickled_python",
    "queue_ops",
    "queue_partitions",
    "queue_blocking_get_wakes_on_put",
    "queue_ephemeral_and_from_id",
    "secret_named_objects",
    "secret_builders",
    "app_lookup_tags_info",
    "app_logs_stream",
    "environments",
    "client_calls",
    "image_builders",
    "image_dockerfile_and_requirements",
    "image_from_id_and_name_and_publish",
    "image_unsupported",
    "image_logs_and_more_builders",
    "file_pattern_matcher",
    "refused_products",
    "workspace_and_environment_administration_is_refused",
    "network_products_are_refused",
    "experimental_app_calls",
}

main()
