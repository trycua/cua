import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

RUNTIME = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(RUNTIME))
import cua  # noqa: E402
from cua import checkpoint  # noqa: E402

JOB = '''
import cua
app = cua.App("demo")
image = cua.Image.python("3.12").uv_pip_install("mlx-lm==0.1", "numpy").pip_install("tqdm")

@app.function(image=image, nodes=(1, 2), accelerator="gpu", retries=cua.Retries(5, 3.0),
              timeout="1h30m", on="host:mac1", env={"A": 1}, kind="vm",
              secrets=[cua.Secret.from_local_env("HF_TOKEN")])
def train(cluster: cua.Cluster):
    import json, os
    keys = ["MASTER_ADDR", "MASTER_PORT", "RANK", "WORLD_SIZE", "NODE_RANK", "LOCAL_RANK", "MLX_RANK"]
    env = {k: os.environ.get(k) for k in keys}
    if os.environ.get("MLX_HOSTFILE"):
        env["hostfile"] = json.load(open(os.environ["MLX_HOSTFILE"]))
    p = cluster.data_dir / "out.txt"
    p.write_text(json.dumps({"rank": cluster.rank, "env": env}))
    return p

@app.function(store="s3://bucket/runs")
def outdir(cluster):
    d = cluster.data_dir / "model"
    d.mkdir()
    (d / "a.bin").write_bytes(b"a")
    return d

@app.function()
def raw():
    return bytes([0, 1])

@app.function()
def launches(cluster):
    import pathlib
    script = pathlib.Path("child.py")
    script.write_text("import os, sys; print('child', os.environ.get('MLX_RANK'), sys.argv[1:]); "
                      "sys.exit(int(os.environ.get('CHILD_RC', '0')))")
    cluster.mlx_launch([str(script), "--x", "1"])

@app.function()
def waits(cluster):
    import pathlib
    script = pathlib.Path("sleeper.py")
    script.write_text("import os, time, pathlib; pathlib.Path('child.pid').write_text(str(os.getpid())); "
                      "time.sleep(120)")
    cluster.mlx_launch([str(script)])

@app.function()
def plain():
    return {"ok": True}

import torch_not_installed_but_unused_ok if False else None
'''


class Parse(unittest.TestCase):
    def test_timeout(self):
        self.assertEqual(cua.parse_timeout("2h"), 7200)
        self.assertEqual(cua.parse_timeout("30m"), 1800)
        self.assertEqual(cua.parse_timeout("1h30m"), 5400)
        self.assertEqual(cua.parse_timeout(90), 90)
        self.assertEqual(cua.parse_timeout("45"), 45)
        for bad in ("soon", "-1", 0, "h"):
            with self.assertRaises(ValueError):
                cua.parse_timeout(bad)

    def test_nodes(self):
        self.assertEqual(cua.parse_nodes(2), {"min": 2, "max": 2})
        self.assertEqual(cua.parse_nodes((1, 2)), {"min": 1, "max": 2})
        self.assertEqual(cua.parse_nodes("1:3"), {"min": 1, "max": 3})
        for bad in ((2, 1), 0, "a"):
            with self.assertRaises(ValueError):
                cua.parse_nodes(bad)

    def test_image_cuda_rewrite(self):
        img = cua.Image.python("3.12").uv_pip_install("mlx-lm==0.32.1", "mlx>=0.30", "numpy")
        self.assertEqual(img.resolved_packages(), ["mlx-lm==0.32.1", "mlx>=0.30", "numpy"])
        self.assertEqual(img.resolved_packages(True),
                         ["mlx-lm==0.32.1", "mlx[cuda]>=0.30", "numpy", "nvidia-cuda-runtime-cu12"])
        # Nothing rewritten: no CUDA extras either.
        self.assertEqual(cua.Image.python().uv_pip_install("numpy").resolved_packages(True), ["numpy"])

    def test_bad_function_args(self):
        app = cua.App("x")
        with self.assertRaises(ValueError):
            app.function(accelerator="tpu")
        with self.assertRaises(ValueError):
            app.function(on="moon")
        with self.assertRaises(ValueError):
            app.function(kind="process")
        with self.assertRaises(ValueError):
            app.function(store="http://x/y")
        with self.assertRaises(ValueError):
            app.function(network="vpn")

    def test_network_direct_sets_the_peer_route(self):
        f = cua.App("x").function(network="direct")(lambda: None)
        self.assertEqual(f.spec["network"], "direct")
        self.assertEqual(f.spec["env"]["CUA_PEER_ROUTE"], "direct")
        g = cua.App("x").function()(lambda: None)
        self.assertEqual(g.spec["network"], "mesh")
        self.assertNotIn("CUA_PEER_ROUTE", g.spec["env"])

    def test_s3_store_takes_the_aws_env_that_is_set(self):
        keep = {k: os.environ.pop(k) for k in cua.AWS_ENV if k in os.environ}
        self.addCleanup(os.environ.update, keep)
        os.environ.update({"AWS_ACCESS_KEY_ID": "AK", "AWS_SECRET_ACCESS_KEY": "SK", "AWS_REGION": "us-west-2"})
        self.addCleanup(lambda: [os.environ.pop(k, None) for k in cua.AWS_ENV])
        f = cua.App("x").function(store="s3://b/p", secrets=[cua.Secret.from_local_env("AWS_REGION")])(
            lambda: None)
        self.assertEqual(f.spec["secrets"], ["AWS_REGION", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"])
        self.assertEqual(f.spec["env"]["CUA_CHECKPOINT_URL"], "s3://b/p")
        g = cua.App("x").function(store="file:///mnt/shared/runs")(lambda: None)
        self.assertEqual(g.spec["secrets"], [])


class CheckpointHelpers(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.d = Path(self.tmp.name)
        self.src = self.d / "src.bin"
        self.src.write_bytes(b"adapter")

    def test_none(self):
        self.assertIsNone(checkpoint.latest(self.d / "missing"))
        self.assertIsNone(checkpoint.latest(self.d))

    def test_save_prune_latest(self):
        for step in (10, 20, 30, 40, 50):
            p = checkpoint.save(self.d / "ck", step, {"adapters.safetensors": self.src})
            self.assertEqual((p / "adapters.safetensors").read_bytes(), b"adapter")
        names = sorted(p.name for p in (self.d / "ck").glob("ckpt-*"))
        self.assertEqual(names, ["ckpt-30", "ckpt-40", "ckpt-50"])
        self.assertEqual(checkpoint.latest(self.d / "ck"), (50, self.d / "ck" / "ckpt-50"))
        man = json.loads((self.d / "ck" / "ckpt-50" / "manifest.json").read_text())
        self.assertEqual(man["files"]["adapters.safetensors"]["size"], 7)

    def test_partial_tmp_ignored(self):
        checkpoint.save(self.d, 10, [self.src])
        (self.d / "ckpt-20.tmp").mkdir()
        self.assertEqual(checkpoint.latest(self.d).step, 10)

    def test_framework_file_names(self):
        for it in (50, 100, 150):
            (self.d / f"{it:07d}_adapters.safetensors").write_bytes(b"x")
        (self.d / "adapters.safetensors").write_bytes(b"final")
        self.assertEqual(checkpoint.latest(self.d, "*_adapters.safetensors").step, 150)

    def test_rejects_path_names(self):
        with self.assertRaises(ValueError):
            checkpoint.save(self.d, 1, {"../x": b"a"})


class Primitives(unittest.TestCase):
    def test_from_env(self):
        env = {"CUA_RANK": "1", "CUA_WORLD_SIZE": "2", "CUA_ATTEMPT": "3", "CUA_RUN_ID": "r1",
               "CUA_PEERS": "10.0.0.1:47000,10.0.0.2:47002"}
        with tempfile.TemporaryDirectory() as d:
            env["CUA_JOB_DATA"] = d
            c = cua.Cluster.from_env(env)
            self.assertEqual((c.rank, c.world_size, c.attempt, c.run_id), (1, 2, 3, "r1"))
            self.assertEqual(c.peers, ["10.0.0.1:47000", "10.0.0.2:47002"])
            self.assertEqual(c.ips, ["10.0.0.1", "10.0.0.2"])
            self.assertEqual((c.master_addr, c.master_port), ("10.0.0.1", 47000))
            self.assertEqual(c.data_dir, Path(d))
            self.assertEqual(c._mlx_hostfile(), [["10.0.0.1:47000"], ["10.0.0.2:47002"]])
            self.assertEqual(c._launch_env(), {
                "MASTER_ADDR": "10.0.0.1", "MASTER_PORT": "47000", "RANK": "1", "WORLD_SIZE": "2",
                "NODE_RANK": "1", "LOCAL_RANK": "0", "LOCAL_WORLD_SIZE": "1", "MLX_RANK": "1"})
            self.assertEqual(c._torchrun_flags(), [
                "--nnodes", "2", "--node_rank", "1", "--nproc_per_node", "1",
                "--master_addr", "10.0.0.1", "--master_port", "47000"])

    def test_public_surface(self):
        public = sorted(n for n in dir(cua.Cluster(data_dir=tempfile.gettempdir())) if not n.startswith("_"))
        self.assertEqual(public, ["attempt", "data_dir", "from_env", "ips", "master_addr", "master_port",
                                  "mlx_launch", "peers", "rank", "run_id", "torchrun", "world_size"])

    def test_world1(self):
        with tempfile.TemporaryDirectory() as d:
            c = cua.Cluster.from_env({"CUA_JOB_DATA": d})
            self.assertEqual((c.master_addr, c.world_size, c.peers), ("127.0.0.1", 1, []))
            self.assertNotIn("MLX_RANK", c._launch_env())

    def test_peer_mismatch(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(ValueError):
                cua.Cluster.from_env({"CUA_WORLD_SIZE": "2", "CUA_PEERS": "a:1", "CUA_JOB_DATA": d})


class LdPath(unittest.TestCase):
    def test_clean(self):
        import _cua_entry as e
        ld = "/usr/local/cuda-13.2/lib:/usr/local/cuda-13.2/lib64:/usr/local/cuda:/opt/amazon/openmpi/lib:/usr/local/lib:/usr/local/cuda-extra"
        self.assertEqual(e.clean_ld_library_path(ld), "/opt/amazon/openmpi/lib:/usr/local/lib:/usr/local/cuda-extra")
        self.assertEqual(e.clean_ld_library_path("/usr/local/cuda/lib64"), "")
        self.assertEqual(e.clean_ld_library_path("/a:/b"), "/a:/b")


class SpaceLine(unittest.TestCase):
    def test_kinds(self):
        import _cua_entry as e
        self.assertIsNone(e.space_line({}))
        vm = e.space_line({"CUA_SPACE_KIND": "vm", "CUA_RANK": "1", "CUA_WORLD_SIZE": "2"})
        self.assertTrue(vm.startswith("rank 1/2 runs in a VM Space, fresh for this attempt (kind=vm, "), vm)
        m = e.space_line({"CUA_SPACE_KIND": "machine"})
        self.assertTrue(m.startswith("rank 0/1 runs in the computer's own Space (a dedicated host) (kind=machine, "), m)
        self.assertIn("kind=container", e.space_line({"CUA_SPACE_KIND": "container"}))


def untar(path):
    import tarfile
    with tarfile.open(path, "r:gz") as t:
        return {m.name: t.extractfile(m).read() for m in t.getmembers() if m.isfile()}


class Entry(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.job = Path(self.tmp.name) / "job.py"
        self.job.write_text(JOB.replace("import torch_not_installed_but_unused_ok if False else None", ""))

    def run_entry(self, *args, **env):
        e = {**os.environ, "PYTHONPATH": str(RUNTIME), **env}
        return subprocess.run([sys.executable, "-m", "_cua_entry", *args], cwd=self.tmp.name,
                              env=e, capture_output=True, text=True, timeout=60)

    def test_spec(self):
        r = self.run_entry("--spec", str(self.job), "train")
        self.assertEqual(r.returncode, 0, r.stderr)
        spec = json.loads(r.stdout)
        self.assertEqual(spec["nodes"], {"min": 1, "max": 2})
        self.assertEqual(spec["timeout_secs"], 5400)
        self.assertEqual(spec["retries"], {"max_retries": 5, "backoff_coefficient": 3.0,
                                           "initial_delay": 2.0, "max_delay": 60.0})
        self.assertEqual((spec["kind"], spec["store"], spec["barrier_timeout_secs"], spec["stall_timeout_secs"],
                          spec["infra_retries"]), ("vm", "", 900, 600, 10))
        self.assertEqual(spec["image"]["packages"], ["mlx-lm==0.1", "numpy", "tqdm"])
        self.assertEqual(spec["secrets"], ["HF_TOKEN"])
        self.assertEqual(spec["env"], {"A": "1"})
        self.assertEqual((spec["on"], spec["accelerator"], spec["file"]), ("host:mac1", "gpu", "job.py"))

    def test_unknown_function(self):
        r = self.run_entry("--spec", str(self.job), "nope")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("available: launches, outdir, plain, raw, train, waits", r.stderr)

    def test_sigterm_stops_the_launched_command(self):
        import signal
        import time
        e = {**os.environ, "PYTHONPATH": str(RUNTIME)}
        p = subprocess.Popen([sys.executable, "-m", "_cua_entry", str(self.job), "waits"], cwd=self.tmp.name,
                             env=e, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        pid_file = Path(self.tmp.name) / "child.pid"
        deadline = time.monotonic() + 30
        while not pid_file.exists() or not pid_file.read_text():
            self.assertLess(time.monotonic(), deadline)
            time.sleep(0.05)
        child = int(pid_file.read_text())
        p.send_signal(signal.SIGTERM)
        self.assertEqual(p.wait(timeout=30), 128 + signal.SIGTERM)
        deadline = time.monotonic() + 10
        while True:
            try:
                os.kill(child, 0)
            except ProcessLookupError:
                break
            if time.monotonic() > deadline:
                os.kill(child, signal.SIGKILL)
                self.fail("the launched command outlived the member")
            time.sleep(0.05)

    def test_store_spec(self):
        r = self.run_entry("--spec", str(self.job), "outdir")
        spec = json.loads(r.stdout)
        self.assertEqual((spec["kind"], spec["store"]), ("", "s3://bucket/runs"))
        self.assertEqual(spec["env"], {"CUA_CHECKPOINT_URL": "s3://bucket/runs"})

    def test_run_rank0_and_rank1(self):
        for rank, writes in ((0, True), (1, False)):
            data = Path(self.tmp.name) / f"data{rank}"
            res = Path(self.tmp.name) / f"result{rank}"
            peers = "127.0.0.1:47000,127.0.0.1:47002"
            r = self.run_entry(str(self.job), "train", CUA_RANK=str(rank), CUA_WORLD_SIZE="2",
                               CUA_PEERS=peers, CUA_JOB_DATA=str(data), CUA_RESULT_PATH=str(res),
                               CUA_RUN_ID="r1")
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual(res.exists(), writes)
            self.assertNotIn("runs in", r.stdout)  # no CUA_SPACE_KIND: not in a Space
            out = json.loads((data / "out.txt").read_text())
            self.assertEqual(out["env"], {
                "MASTER_ADDR": "127.0.0.1", "MASTER_PORT": "47000", "RANK": str(rank), "WORLD_SIZE": "2",
                "NODE_RANK": str(rank), "LOCAL_RANK": "0", "MLX_RANK": str(rank),
                "hostfile": [["127.0.0.1:47000"], ["127.0.0.1:47002"]]})
            if writes:
                self.assertEqual(json.loads(untar(res)["out.txt"])["rank"], 0)

    def test_member_says_where_it_runs(self):
        data = Path(self.tmp.name) / "data-k"
        r = self.run_entry(str(self.job), "train", CUA_SPACE_KIND="container", CUA_JOB_DATA=str(data),
                           CUA_RUN_ID="r1")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("cua: rank 0/1 runs in a container Space, fresh for this attempt (kind=container", r.stdout)

    def test_directory_and_bytes_results(self):
        res = Path(self.tmp.name) / "result"
        d = Path(self.tmp.name) / "d"
        r = self.run_entry(str(self.job), "outdir", CUA_JOB_DATA=str(d), CUA_RESULT_PATH=str(res),
                           CUA_CHECKPOINT_URL="file://" + str(Path(self.tmp.name) / "store"))
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(untar(res), {"model/a.bin": b"a"})
        r = self.run_entry(str(self.job), "raw", CUA_JOB_DATA=str(d), CUA_RESULT_PATH=str(res))
        self.assertEqual(untar(res), {"result.bin": b"\x00\x01"})

    def test_mlx_launch_runs_the_command_as_this_rank(self):
        env = dict(CUA_RANK="1", CUA_WORLD_SIZE="2", CUA_PEERS="127.0.0.1:47000,127.0.0.1:47002",
                   CUA_JOB_DATA=str(Path(self.tmp.name) / "d"))
        r = self.run_entry(str(self.job), "launches", **env)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("child 1 ['--x', '1']", r.stdout)
        r = self.run_entry(str(self.job), "launches", CHILD_RC="4", **env)
        self.assertEqual(r.returncode, 1)
        self.assertIn("cua: child.py exited with status 4", r.stderr)

    def test_non_file_result_is_json(self):
        res = Path(self.tmp.name) / "result"
        r = self.run_entry(str(self.job), "plain", CUA_JOB_DATA=str(Path(self.tmp.name) / "d"),
                           CUA_RESULT_PATH=str(res))
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(json.loads(untar(res)["result.json"]), {"ok": True})
        self.assertFalse((Path(self.tmp.name) / "d" / "result.json").exists())  # not in data_dir

    def test_launch_picks_cuda_builds_on_nvidia_hosts_only(self):
        import _cua_entry as e
        env = {"CUA_JOB_PYTHON": "3.11", "CUA_JOB_PACKAGES": json.dumps(["mlx==0.32.4", "mlx-lm==0.32.1"])}
        cpu = e.launch_argv("train.py", "finetune", env, nvidia=False)
        self.assertEqual(cpu, ["uv", "run", "--no-project", "--python", "3.11",
                               "--with", "mlx==0.32.4", "--with", "mlx-lm==0.32.1",
                               "python", "-m", "_cua_entry", "train.py", "finetune"])
        gpu = e.launch_argv("train.py", "finetune", env, nvidia=True)
        self.assertIn("mlx[cuda]==0.32.4", gpu)
        self.assertIn("nvidia-cuda-runtime-cu12", gpu)
        self.assertIn("mlx-lm==0.32.1", gpu)
        self.assertEqual(e.launch_argv("t.py", "f", {}, nvidia=True)[4:6], ["3.12", "python"])

    def test_failure_exit_code(self):
        bad = Path(self.tmp.name) / "bad.py"
        bad.write_text("import cua\napp = cua.App('b')\n@app.function()\ndef f():\n    raise RuntimeError('boom')\n")
        r = self.run_entry(str(bad), "f")
        self.assertEqual(r.returncode, 1)
        self.assertIn("boom", r.stderr)


if __name__ == "__main__":
    unittest.main()
