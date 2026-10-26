"""Functions the hermetic Function suite calls (imported by the client and,
in the container, by Modal's own entrypoint)."""

import os
import time

import modal

app = modal.App("hermetic-functions")
IMG = modal.Image.debian_slim(python_version="3.12")
COUNT = modal.Secret.from_dict({"COUNT_DIR": "/tmp"})


@app.function(image=IMG)
def square(x):
    return x * x


@app.function(image=IMG)
def whoami():
    return os.environ.get("MODAL_TASK_ID", ""), os.environ.get("MODAL_IS_REMOTE")


@app.function(image=IMG)
def boom(msg):
    raise ValueError(msg)


@app.function(image=IMG)
def gen(n):
    for i in range(n):
        yield i * 10


@app.function(image=IMG, timeout=2)
def slow():
    time.sleep(30)


@app.function(
    image=IMG,
    secrets=[COUNT],
    retries=modal.Retries(max_retries=3, initial_delay=0.1, backoff_coefficient=1.0),
)
def flaky(name):
    path = os.path.join(os.environ["COUNT_DIR"], name)
    n = int(open(path).read()) if os.path.exists(path) else 0
    open(path, "w").write(str(n + 1))
    if n < 2:
        raise RuntimeError(f"attempt {n}")
    return n


@app.function(image=IMG)
def spawned(x):
    time.sleep(0.5)
    return x + 1


@app.cls(image=IMG)
class Counter:
    base: int = modal.parameter()

    @modal.enter()
    def setup(self):
        self.started = time.time()

    @modal.method()
    def add(self, x):
        return self.base + x

    @modal.method()
    def twice(self, x):
        yield self.base + x
        yield self.base + x


@app.function(image=IMG, secrets=[modal.Secret.from_dict({"GREETING": "hi"})])
def secret_env():
    return os.environ.get("GREETING")


@app.function(image=IMG)
@modal.batched(max_batch_size=4, wait_ms=500)
def batch_double(xs: list[int]) -> list[int]:
    return [x * 2 for x in xs]


@app.function(image=IMG, max_containers=1)
@modal.concurrent(max_inputs=4)
def concurrent_sleep(t):
    time.sleep(t)
    return os.getpid()


@app.function(image=IMG)
def input_ids():
    return modal.current_input_id(), modal.current_function_call_id()


@app.function(image=IMG)
def is_local_here():
    return modal.is_local()


@app.function(
    image=IMG,
    env={"FROM_ENV": "yes"},
    secrets=[modal.Secret.from_dict({"S": "s"})],
    cpu=0.5,
    memory=512,
)
def resources_env():
    return os.environ.get("FROM_ENV"), os.environ.get("S")


@app.cls(image=IMG, scaledown_window=60)
class Lifecycle:
    @modal.enter()
    def up(self):
        self.msg = "entered"

    @modal.exit()
    def down(self):
        print("exit-hook-ran")

    @modal.method()
    def hello(self):
        return self.msg


@app.function(
    image=IMG, volumes={"/data": modal.Volume.from_name("fn-vol", create_if_missing=True)}
)
def writes_volume(text):
    open("/data/out.txt", "w").write(text)
    return open("/data/out.txt").read()


@app.local_entrypoint()
def main(n: int = 3):
    print("local-entrypoint", sum(square.map(range(n))))


def write_marker(path, text, suffix=""):
    """An Image.run_function build step."""
    with open(path, "w") as f:
        f.write(text + suffix)


def failing_build_step():
    raise RuntimeError("build step blew up")


@app.function(image=IMG)
def echo(x):
    return x


@app.function(image=IMG)
def big_gen(n):
    for _ in range(n):
        yield b"y" * (3 * 1024 * 1024)
