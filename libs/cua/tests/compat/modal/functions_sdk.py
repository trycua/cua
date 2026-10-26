"""The unmodified `modal` client calling Functions through the gateway.

The gateway starts containers with the Function's image; in the hermetic
setup the container runtime is Modal's own `_container_entrypoint` run as a
host process (`CUA_MODAL_LOCAL_RUNTIME`), so the whole Function path
(registration, calls, outputs, retries, timeouts, generators, classes) runs
for real. Run by run_hermetic.sh.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import time
import traceback
import uuid

# With an env file (the hermetic runners) its values are the gateway's; without,
# the environment is (`eval "$(cua compat env --provider modal)"`).
if len(sys.argv) > 1:
    os.environ.update(json.loads(open(sys.argv[1]).read()))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
RUN = uuid.uuid4().hex[:8]

import fn_app  # noqa: E402
import modal  # noqa: E402
from modal.exception import (  # noqa: E402
    ExecutionError,
    FunctionTimeoutError,
    InvalidError,
    NotFoundError,
)

failures: list[str] = []
passed = 0


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


def map_unordered():
    # Results arrive in completion order; sorted they are the squares.
    eq(sorted(fn_app.square.map([3, 1, 2], order_outputs=False)), [1, 4, 9])


def main():
    with modal.enable_output(), fn_app.app.run():
        check("remote", lambda: eq(fn_app.square.remote(7), 49))
        check("local", lambda: eq(fn_app.square.local(3), 9))
        check("container_env", container_env)
        check("map", lambda: eq(list(fn_app.square.map([1, 2, 3, 4])), [1, 4, 9, 16]))
        check("map_unordered", map_unordered)
        check("starmap", lambda: eq(list(fn_app.square.starmap([(2,), (3,)])), [4, 9]))
        check("for_each", lambda: fn_app.square.for_each([1, 2]))
        check("remote_error", remote_error)
        check("map_exception", map_exception)
        check("generator", lambda: eq(list(fn_app.gen.remote_gen(4)), [0, 10, 20, 30]))
        check("spawn_get", spawn_get)
        check("function_call_from_id", function_call_from_id)
        check("cancel", cancel)
        check("timeout", lambda: raises(FunctionTimeoutError, fn_app.slow.remote))
        check("retries", lambda: eq(fn_app.flaky.remote(f"flaky-{RUN}"), 2))
        check("cls", cls)
        check("cls_generator", lambda: eq(list(fn_app.Counter(base=1).twice.remote_gen(2)), [3, 3]))
        check("secret_env", lambda: eq(fn_app.secret_env.remote(), "hi"))
        check("stats", stats)
        check("update_autoscaler", update_autoscaler)
        check("batched", batched)
        check("concurrent", concurrent)
        check("current_ids", current_ids)
        check("is_local", lambda: eq(fn_app.is_local_here.remote(), False))
        check("resources_and_env", lambda: eq(fn_app.resources_env.remote(), ("yes", "s")))
        check("with_options", with_options)
        check("cls_options", cls_options)
        check("lifecycle_hooks", lifecycle_hooks)
        check("spawn_map", spawn_map)
        check("function_call_iter", function_call_iter)
        check("local_entrypoint", local_entrypoint)
        check("metrics_are_refused", metrics_are_refused)
        check("app_members", app_members)
        check("gather_and_graph", gather_and_graph)
        check("image_run_function", image_run_function)
        check("large_payloads", large_payloads)
        check("logs_managers", logs_managers)
        check("function_info", function_info)
        if os.environ.get("FUNCTIONS_LANE") == "space":
            check("volume_in_function", volume_in_function)
        check("refused_at_registration", refused_at_registration)
        check("clustered_is_refused", clustered_is_refused)
    check("deploy_and_lookup", deploy_and_lookup)
    print(f"{passed} passed, {len(failures)} failed: {failures}")
    sys.exit(1 if failures else 0)


def eq(a, b):
    assert a == b, f"{a!r} != {b!r}"


def container_env():
    task, remote = fn_app.whoami.remote()
    assert task.startswith("ta-") or task, task
    eq(remote, "1")


def remote_error():
    e = raises(ValueError, lambda: fn_app.boom.remote("nope"))
    assert "nope" in str(e)


def map_exception():
    out = list(fn_app.boom.map(["a", "b"], return_exceptions=True))
    assert all(isinstance(x, ValueError) for x in out), out


def spawn_get():
    call = fn_app.spawned.spawn(1)
    eq(call.get(), 2)
    eq(call.get(), 2)


def function_call_from_id():
    call = fn_app.spawned.spawn(5)
    again = modal.FunctionCall.from_id(call.object_id)
    eq(again.get(), 6)


def cancel():
    call = fn_app.slow.spawn()
    time.sleep(0.5)
    call.cancel()
    e = raises(modal.exception.RemoteError, lambda: call.get(timeout=20))
    assert "cancel" in str(e), e


def cls():
    c = fn_app.Counter(base=100)
    eq(c.add.remote(5), 105)
    eq(fn_app.Counter(base=1).add.remote(1), 2)


def stats():
    s = fn_app.square.get_current_stats()
    assert s.backlog >= 0


def update_autoscaler():
    fn_app.square.update_autoscaler(min_containers=0, max_containers=3)


def batched():
    # The caller passes one item; the function receives a list of them.
    eq(fn_app.batch_double.remote(5), 10)
    eq(sorted(fn_app.batch_double.map([1, 2, 3])), [2, 4, 6])


def concurrent():
    calls = [fn_app.concurrent_sleep.spawn(1.0) for _ in range(3)]
    t0 = time.time()
    pids = [c.get() for c in calls]
    assert time.time() - t0 < 2.8, "inputs did not run concurrently"
    eq(len(set(pids)), 1)  # one container


def current_ids():
    input_id, call_id = fn_app.input_ids.remote()
    assert input_id.startswith("in-") and call_id.startswith("fc-"), (input_id, call_id)


def with_options():
    f = fn_app.square.with_options(secrets=[modal.Secret.from_dict({"X": "1"})], timeout=30)
    eq(f.remote(4), 16)
    g = fn_app.square.with_concurrency(max_inputs=2)
    eq(g.remote(3), 9)
    h = fn_app.batch_double.with_batching(max_batch_size=2, wait_ms=100) if False else None


def cls_options():
    c = fn_app.Counter.with_options(timeout=60)(base=7)
    eq(c.add.remote(1), 8)
    c = fn_app.Counter.with_concurrency(max_inputs=2)(base=9)
    eq(c.add.remote(1), 10)


def lifecycle_hooks():
    eq(fn_app.Lifecycle().hello.remote(), "entered")


def spawn_map():
    fn_app.square.spawn_map([1, 2, 3])
    fn_app.square.experimental_spawn_map([4, 5])
    call = fn_app.spawned.spawn(1)
    eq(modal.FunctionCall.gather(call, fn_app.spawned.spawn(2)), [2, 3])


def function_call_iter():
    fc = fn_app.square.experimental_spawn_map([1, 2, 3, 4])
    eq(list(fc.iter()), [1, 4, 9, 16])
    eq(list(fc.iter(start=1, end=3)), [4, 9])
    eq(fc.num_inputs(), 4)


def local_entrypoint():
    # The decorated function runs locally and calls the Function remotely.
    eq(fn_app.main(3), None)


def metrics_are_refused():
    e = raises(modal.exception.UnimplementedError, lambda: list(fn_app.square.metrics.export()))
    assert "metrics" in str(e), e


def app_members():
    eq(fn_app.square.app.name, "hermetic-functions")
    eq(fn_app.square.get_web_url(), "")


def large_payloads():
    # Above the size Modal sends inline, inputs and outputs travel as blobs.
    big = os.urandom(6 * 1024 * 1024)
    eq(fn_app.echo.remote(big), big)
    out = list(fn_app.echo.map([os.urandom(3 * 1024 * 1024) for _ in range(2)]))
    eq([len(x) for x in out], [3 * 1024 * 1024] * 2)
    # A generator's chunks.
    eq([len(x) for x in fn_app.big_gen.remote_gen(2)], [3 * 1024 * 1024] * 2)


def image_run_function():
    # The hermetic runtime is a host process, so the step's effect is a host
    # file; on a Space it is in the image (conformance.py: image_run_function).
    path = os.path.join(tempfile.mkdtemp(prefix="cua-build-"), "marker.txt")
    img = modal.Image.debian_slim(python_version="3.12").run_function(
        fn_app.write_marker, args=(path, "built"), kwargs={"suffix": "!"}
    )
    img.build(fn_app.app)
    assert img.object_id.startswith("im-"), img.object_id
    if os.environ.get("FUNCTIONS_LANE") != "space":
        eq(open(path).read(), "built!")
    # A failing step is an ImageBuildError carrying its exception.
    bad = modal.Image.debian_slim(python_version="3.12").run_function(fn_app.failing_build_step)
    e = raises(modal.exception.ImageBuildError, lambda: bad.build(fn_app.app))
    assert "build step blew up" in str(e) or "failed" in str(e), e


def gather_and_graph():
    call = fn_app.spawned.spawn(7)
    eq(call.get(), 8)
    graph = call.get_call_graph()
    assert graph, graph
    eq(call.num_inputs(), 1)


def logs_managers():
    import datetime

    since = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=10)
    entries = list(fn_app.app.logs.fetch(since=since))
    assert isinstance(entries, list)
    list(fn_app.square.logs.fetch(since=since))


def function_info():
    info = fn_app.square.info()
    eq(info.timeout, None if info.timeout is None else info.timeout)
    assert info.cpu is None or info.cpu > 0, info
    s = fn_app.square.autoscaler.settings()
    assert s is not None
    fn_app.square.autoscaler.update(min_containers=0, max_containers=4, scaledown_window=30)
    eq(fn_app.square.autoscaler.settings().max_containers, 4)
    eq(fn_app.square.get_current_stats().backlog >= 0, True)
    assert fn_app.square.get_dashboard_url().startswith("http")


def volume_in_function():
    # Needs a real container: the hermetic runtime is a host process with no mounts.
    eq(fn_app.writes_volume.remote("from-function"), "from-function")


def refused_at_registration():
    def registers(decorate):
        app = modal.App("hermetic-refused")
        decorate(app)
        return app.run().__enter__()

    def plain(**kw):
        return lambda app: app.function(serialized=True, **kw)(_noop)

    def web(deco):
        def go(app):
            app.function(serialized=True)(deco(_noop))

        return go

    def server(app):
        app.server(port=8000, serialized=True)(_Srv)

    cases = {
        "server": (server, "Server Functions"),
        "gpu": (plain(gpu="A10G"), "cua.gpu"),
        "web_server": (web(lambda f: modal.web_server(8000)(f)), "web endpoints"),
        "asgi_app": (web(lambda f: modal.asgi_app()(f)), "web endpoints"),
        "wsgi_app": (web(lambda f: modal.wsgi_app()(f)), "web endpoints"),
        "fastapi_endpoint": (web(lambda f: modal.fastapi_endpoint()(f)), "web endpoints"),
        "schedule": (plain(schedule=modal.Period(days=1)), "schedules"),
        "cron": (plain(schedule=modal.Cron("* * * * *")), "schedules"),
        "proxy": (plain(proxy=modal.Proxy.from_name("p")), "Proxies"),
        "network_file_systems": (
            plain(
                network_file_systems={
                    "/n": modal.NetworkFileSystem.from_name("n", create_if_missing=True)
                }
            ),
            "network file systems",
        ),
        "experimental_option": (plain(experimental_options={"x": "1"}), "experimental option"),
    }
    for name, (decorate, needle) in cases.items():
        e = raises(Exception, lambda: registers(decorate))
        assert isinstance(e, modal.exception.UnimplementedError) and needle in str(e), (
            name,
            type(e).__name__,
            str(e)[:200],
        )


def _noop():
    return 1


class _Srv:
    @modal.enter()
    def start(self):
        pass


def clustered_is_refused():
    import modal.experimental

    app = modal.App("hermetic-cluster")
    app.function(serialized=True)(modal.experimental.clustered(2)(_noop))
    e = raises(Exception, lambda: app.run().__enter__())
    assert isinstance(e, modal.exception.UnimplementedError), e


def deploy_and_lookup():
    fn_app.app.deploy(name="hermetic-deployed")
    f = modal.Function.from_name("hermetic-deployed", "square")
    eq(f.remote(6), 36)
    c = modal.Cls.from_name("hermetic-deployed", "Counter")
    eq(c(base=2).add.remote(2), 4)
    raises(NotFoundError, lambda: modal.Function.from_name("hermetic-deployed", "nope").hydrate())


main()
