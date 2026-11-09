"""bench-web adapters, LLM judges, pinned data and grounding sets (hermetic).

A fake bench-web-ctl, a fake OpenAI endpoint and synthetic grounding items;
no network, sandbox or API key.
"""

from __future__ import annotations

import asyncio
import importlib.util
import io
import json
import threading
import urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace

import pytest
from cua_bench.adapters import Endpoints, datasets, judges
from cua_bench.computers.dataset import DatasetSession
from cua_bench.targets import Target, resolve_env_spec

PKG = Path(__file__).resolve().parents[2]


def _load(rel: str, name: str):
    spec = importlib.util.spec_from_file_location(name, PKG / rel)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _png(w=100, h=80) -> bytes:
    from PIL import Image

    buf = io.BytesIO()
    Image.new("RGB", (w, h)).save(buf, format="PNG")
    return buf.getvalue()


# ── a fake bench-web-ctl ──────────────────────────────────────────────────


class FakeCtl:
    def __init__(self):
        self.calls: list[tuple[str, dict]] = []
        self.values: dict = {}
        ctl = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_GET(self):
                self._send(200, {"ok": True})

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["content-length"] or 0)) or b"{}")
                ctl.calls.append((self.path, body))
                if self.path == "/eval":
                    expr = body["expression"]
                    for key, value in ctl.values.items():
                        if key in expr:
                            return self._send(200, {"value": value})
                    return self._send(200, {"value": None})
                self._send(200, {"url": body.get("url"), "title": ""})

            def _send(self, code, obj):
                data = json.dumps(obj).encode()
                self.send_response(code)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()


class _Svc:
    def __init__(self, port):
        self.port = port

    async def url(self):
        return f"http://127.0.0.1:{self.port}"

    async def request(self, method, path, json=None, timeout=30, **_):
        import httpx

        async with httpx.AsyncClient(timeout=timeout) as c:
            return await c.request(method, f"http://127.0.0.1:{self.port}{path}", json=json)


@pytest.fixture
def ctl():
    c = FakeCtl()
    yield c
    c.server.shutdown()


def _ep(ctl, adapter):
    sb = SimpleNamespace(exposed_ports={}, service=lambda name: _Svc(ctl.port))
    session = SimpleNamespace(sandbox=sb, screenshot=lambda: None)
    return session, Endpoints(session, adapter.server)


# ── MiniWoB ───────────────────────────────────────────────────────────────


def test_miniwob_tasks_setup_eval_oracle(ctl, monkeypatch):
    monkeypatch.setenv("CUA_BENCH_MINIWOB_TASKS", "click-button,enter-text")
    monkeypatch.setenv("CUA_BENCH_MINIWOB_SEEDS", "2")
    mod = _load("tasks/miniwob/main.py", "miniwob_main")
    assert len(mod.TASKS) == 130 and "click-button" in mod.TASKS
    tasks = mod.load()
    assert [t.task_id for t in tasks] == ["miniwob-click-button-s0", "miniwob-click-button-s1",
                                         "miniwob-enter-text-s0", "miniwob-enter-text-s1"]
    setup = tasks[0].computer["setup_config"]
    assert setup["server_port"] == 7000 and setup["ports"] == [9222, 7560]
    assert setup["image"].startswith("ghcr.io/trycua/bench-web") and setup["requires"] == []
    adapter = mod.ADAPTER
    session, ep = _ep(ctl, adapter)
    ctl.values["startEpisodeReal"] = 'Click on the "OK" button.'
    ctl.values["WOB_DONE_GLOBAL"] = {"done": True, "raw": 1.0, "reward": 0.97, "reason": None}
    task = tasks[0]

    async def run():
        await adapter.setup(task, session, ep)
        await adapter.oracle(task, session, ep)
        return await adapter.evaluate(task, session, ep)

    assert asyncio.run(run()) == 1.0
    assert task.description.startswith('Click on the "OK" button.')
    paths = [p for p, _ in ctl.calls]
    assert paths[0] == "/reset" and ctl.calls[0][1]["url"].endswith("/miniwob/click-button.html")
    assert "Math.seedrandom(\"0\")" in ctl.calls[1][1]["expression"]
    assert "600000" in ctl.calls[1][1]["expression"]
    assert mod.score({"done": True, "raw": -1}) == 0.0 and mod.score({"done": False, "raw": 1}) == 0.0
    with pytest.raises(ValueError, match="unknown MiniWoB"):
        monkeypatch.setenv("CUA_BENCH_MINIWOB_TASKS", "nope")
        mod.load()


# ── judges ────────────────────────────────────────────────────────────────


class FakeChat:
    def __init__(self, *answers):
        self.answers = list(answers)
        self.calls = []

    async def __call__(self, messages):
        self.calls.append(messages)
        return self.answers.pop(0) if len(self.answers) > 1 else self.answers[0]


def test_webvoyager_judge_rule():
    shot = _png()
    assert asyncio.run(judges.webvoyager("t", "a", [shot], FakeChat("... SUCCESS"))) == 1.0
    assert asyncio.run(judges.webvoyager("t", "a", [shot], FakeChat("NOT SUCCESS"))) == 0.0
    assert asyncio.run(judges.webvoyager("t", "a", [shot], FakeChat("unsure"))) is None
    chat = FakeChat("SUCCESS")
    asyncio.run(judges.webvoyager("find x", "x is 3", [shot, shot], chat))
    user = chat.calls[0][1]["content"]
    assert user[0]["text"] == "TASK: find x\nResult Response: x is 3\n2 screenshots at the end: "
    assert sum(1 for c in user if c["type"] == "image_url") == 2


def test_webjudge_flow():
    chat = FakeChat("**Key Points**:\n1. Filter by price", "**Reasoning**: shows filter\n**Score**: 4",
                    'Thoughts: ok\nStatus: "success"')
    assert asyncio.run(judges.webjudge("buy x", ["click a"], [_png()], chat)) == 1.0
    final = chat.calls[-1][1]["content"]
    assert "1. click a" in final[0]["text"] and any(c["type"] == "image_url" for c in final)
    chat = FakeChat("Key Points: 1. a", "**Score**: 1", 'Status: "failure"')
    assert asyncio.run(judges.webjudge("t", [], [_png()], chat)) == 0.0


def test_webgym_rubric():
    ref = [{"description": "find event", "facts": ["date", "location"]}]
    ok = FakeChat("2. Verdict: SUCCESS")
    assert asyncio.run(judges.webgym("t", ref, "on May 1 in LA", [], [_png()], ok)) == 1.0
    assert len(ok.calls) == 3  # two facts + the answer check
    mixed = FakeChat("2. Verdict: SUCCESS", "2. Verdict: NOT SUCCESS", "3. Verdict: SUCCESS")
    assert asyncio.run(judges.webgym("t", ref, "x", [], [_png()], mixed)) == 0.0


def test_openai_chat_retries_rate_limits(monkeypatch):
    hits = []

    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["content-length"])))
            hits.append((self.headers["authorization"], body["model"]))
            code = 429 if len(hits) == 1 else 200
            data = json.dumps({"choices": [{"message": {"content": "SUCCESS"}}]}).encode()
            self.send_response(code)
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    monkeypatch.setenv("OPENAI_API_KEY", "sk-test")
    monkeypatch.setenv("OPENAI_BASE_URL", f"http://127.0.0.1:{srv.server_address[1]}/v1")
    real_sleep = asyncio.sleep
    monkeypatch.setattr(judges.asyncio, "sleep", lambda s: real_sleep(0))
    chat = judges.OpenAIChat("gpt-4.1")
    assert asyncio.run(chat([{"role": "user", "content": "hi"}])) == "SUCCESS"
    assert hits == [("Bearer sk-test", "gpt-4.1")] * 2
    srv.shutdown()
    monkeypatch.delenv("OPENAI_API_KEY")
    with pytest.raises(RuntimeError, match="OPENAI_API_KEY"):
        judges.OpenAIChat("x")


# ── live-web adapters ─────────────────────────────────────────────────────


def test_webvoyager_adapter(ctl, tmp_path):
    data = tmp_path / "wv.jsonl"
    data.write_text(json.dumps({"web_name": "ArXiv", "id": "ArXiv--3", "ques": "Find X",
                                "web": "https://arxiv.org/"}) + "\n")
    mod = _load("tasks/webvoyager/main.py", "wv_main")
    adapter = mod.WebVoyagerAdapter(chat=FakeChat("SUCCESS"), data=str(data))
    (task,) = adapter.tasks("train")
    setup = task.computer["setup_config"]
    assert setup["requires"] == ["egress", "openai"]
    session, ep = _ep(ctl, adapter)

    async def shot():
        return _png()

    session.screenshot = shot
    session.final_answer = "X is 42"

    async def run():
        await adapter.setup(task, session, ep)
        return await adapter.evaluate(task, session, ep)

    assert asyncio.run(run()) == 1.0
    assert ctl.calls[0] == ("/reset", {"url": "https://arxiv.org/"})
    assert adapter._chat.calls[0][1]["content"][0]["text"].startswith("TASK: Find X\nResult Response: X is 42")


def test_webgym_and_om2w_loaders(tmp_path, monkeypatch):
    wg = tmp_path / "test.jsonl"
    wg.write_text(json.dumps({"task_name": "Who wrote Y?", "website": "https://y.example", "difficulty": 2,
                              "evaluator_reference": [{"id": 1, "description": "d", "facts": ["f"]}],
                              "definite_answer": "", "task_id": "7"}) + "\n")
    mod = _load("tasks/webgym/main.py", "wg_main")
    (task,) = mod.WebGymAdapter(data=str(wg)).tasks("train")
    assert task.metadata["start_url"] == "https://y.example" and task.metadata["reference"][0]["facts"] == ["f"]
    om = tmp_path / "om.json"
    om.write_text(json.dumps([{"task_id": "a1", "confirmed_task": "Book Z", "website": "https://z",
                               "level": "easy"}]))
    om_mod = _load("tasks/online_mind2web/main.py", "om_main")
    (t,) = om_mod.OnlineMind2WebAdapter(data=str(om)).tasks("train")
    assert t.description == "Book Z" and t.computer["setup_config"]["requires"] == ["egress", "hf-gated", "openai"]


def test_gated_dataset_names_the_access_page(monkeypatch, tmp_path):
    monkeypatch.setenv("CUA_BENCH_CACHE", str(tmp_path))
    monkeypatch.setenv("HF_TOKEN", "hf_x")

    def denied(url, dest, sha256=None, headers=None, timeout=300):
        raise urllib.error.HTTPError(url, 403, "Forbidden", {}, None)

    monkeypatch.setattr(datasets, "fetch_url", denied)
    with pytest.raises(datasets.GatedDatasetError, match="huggingface.co/datasets/osunlp/Online-Mind2Web"):
        datasets.hf_file("osunlp/Online-Mind2Web", "x.json", "0" * 40, gated=True)
    monkeypatch.delenv("HF_TOKEN")
    monkeypatch.setattr(datasets.shutil, "which", lambda name: None)
    with pytest.raises(datasets.GatedDatasetError, match="request access"):
        datasets.hf_file("osunlp/Online-Mind2Web", "x.json", "0" * 40, gated=True)


def test_fetch_url_verifies_sha256(tmp_path, monkeypatch):
    src = tmp_path / "src.txt"
    src.write_text("hello")
    url = src.as_uri()
    good = __import__("hashlib").sha256(b"hello").hexdigest()
    assert datasets.fetch_url(url, tmp_path / "a" / "x", good).read_text() == "hello"
    with pytest.raises(RuntimeError, match="sha256"):
        datasets.fetch_url(url, tmp_path / "b" / "x", "0" * 64)
    assert not (tmp_path / "b" / "x").exists()


# ── grounding ─────────────────────────────────────────────────────────────


def _grounding_items(tmp_path):
    images = tmp_path / "images"
    images.mkdir()
    (images / "a.png").write_bytes(_png(200, 100))
    items = [
        {"id": "A-0", "image_path": "a.png", "image_size": [200, 100], "instruction": "Click OK",
         "box_type": "bbox", "box_coordinates": [10, 10, 20, 10], "GUI_types": ["Button"]},
        {"id": "A-1", "image_path": "a.png", "image_size": [200, 100], "instruction": "Close",
         "box_type": "polygon", "box_coordinates": [100, 10, 120, 10, 120, 30, 100, 30], "GUI_types": []},
        {"id": "A-2", "image_path": "a.png", "image_size": [200, 100], "instruction": "Click Cindy",
         "box_type": "refusal", "box_coordinates": [0, 0, 0, 0], "GUI_types": []},
    ]
    data = tmp_path / "g.json"
    data.write_text(json.dumps(items))
    return str(data), str(images)


def test_osworld_g(tmp_path):
    data, images = _grounding_items(tmp_path)
    mod = _load("tasks/osworld_g/main.py", "og_main")
    adapter = mod.OSWorldGAdapter(data=data, images_dir=images)
    tasks = adapter.tasks("train")
    assert tasks[0].computer["provider"] == "dataset"
    assert resolve_env_spec(tasks[0].computer, Target(on="cloud")).needs_sandbox is False

    async def episode(task, act):
        s = DatasetSession()
        await adapter.setup(task, s, None)
        assert (s.width, s.height) == (200, 100)
        await act(s)
        return await adapter.evaluate(task, s, None)

    async def noop(s):
        pass

    for task in tasks:
        assert asyncio.run(episode(task, lambda s, t=task: adapter.oracle(t, s, None))) == 1.0
        assert asyncio.run(episode(task, noop)) == 0.0

    async def click_then_refuse(s):
        await s.click(1, 1)
        await s.report_infeasible()

    assert asyncio.run(episode(tasks[2], click_then_refuse)) == 0.0  # refusal with a click
    from cua_bench.adapters.grounding import in_polygon, polygon_centroid

    assert in_polygon((110, 20), [100, 10, 120, 10, 120, 30, 100, 30])
    assert not in_polygon((99, 20), [100, 10, 120, 10, 120, 30, 100, 30])
    assert in_polygon((100, 20), [100, 10, 120, 10, 120, 30, 100, 30])  # on the edge
    assert polygon_centroid([0, 0, 2, 0, 2, 2, 0, 2]) == (1.0, 1.0)


def test_screenspot_pro_exactly_one_click(tmp_path):
    ann = tmp_path / "ann"
    ann.mkdir()
    (ann / "excel_macos.json").write_text(json.dumps([
        {"img_filename": "x/a.png", "bbox": [10, 10, 30, 30], "instruction": "Pick", "id": "excel_macos_0",
         "application": "excel", "platform": "macos", "img_size": [200, 100], "ui_type": "text",
         "group": "Office"}]))
    (tmp_path / "img" / "x").mkdir(parents=True)
    (tmp_path / "img" / "x" / "a.png").write_bytes(_png(200, 100))
    mod = _load("tasks/screenspot_pro/main.py", "ss_main")
    adapter = mod.ScreenSpotProAdapter(annotations_dir=str(ann), images_dir=str(tmp_path / "img"))
    import os

    os.environ["CUA_BENCH_SSPRO_APPS"] = "excel_macos"
    try:
        (task,) = adapter.tasks("train")
    finally:
        del os.environ["CUA_BENCH_SSPRO_APPS"]

    async def episode(clicks):
        s = DatasetSession()
        await adapter.setup(task, s, None)
        for x, y in clicks:
            await s.click(x, y)
        return await adapter.evaluate(task, s, None)

    assert asyncio.run(episode([(20, 20)])) == 1.0
    assert asyncio.run(episode([(20, 20), (20, 20)])) == 0.0  # exactly one point action
    assert asyncio.run(episode([(50, 50)])) == 0.0


# ── cua-bench-basic on bench-web ──────────────────────────────────────────


def test_basic_tasks_run_on_bench_web(monkeypatch):
    from cua_bench import images

    for name in ("click-button", "video-player", "drag-drop"):
        mod = _load(f"datasets/cua-bench-basic/{name}/main.py", f"basic_{name}")
        task = mod.load()[0]
        assert task.computer["setup_config"]["image"] == images.BENCH_WEB
        assert resolve_env_spec(task.computer, Target()).image == images.BENCH_WEB
    monkeypatch.setenv("CUA_BENCH_IMAGE_BENCH_WEB", "registry.example/bench-web@sha256:" + "1" * 64)
    mod = _load("datasets/cua-bench-basic/click-button/main.py", "basic_override")
    assert mod.load()[0].computer["setup_config"]["image"].startswith("registry.example/")
