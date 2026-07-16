#!/usr/bin/env python3
"""Live demo — N lanes loop ``square -> triangle -> RESET`` forever, streamed to
a local webserver (one live MJPEG per lane) with a metadata overlay.

What it illustrates (cyclops-cs 0.2.0):
  * explicit ``acquire_lanes`` / ``release_lanes``
  * the SAME warm VM reused across batches — square (batch 1) then triangle
    (batch 2) land on one desktop; the on-frame ``vm`` id stays the same
  * in-band ``RESET`` (batch 3) hard-resets the VM — watch the ``vm`` id change
  * the generic facade — every call is ``client.<op>(...)``, no ``.sync``

Each frame carries a banner (lane, iteration, phase, step, vm id); while a batch
is being provisioned/run you get a loading screen with an elapsed timer.

Setup::

    pip install cua-train --extra-index-url https://wheels.cua.ai/simple/
    pip install pillow boto3
    export CUA_CLIENT_ID=key-xxxxxxxx      # from POST /api/keys
    export CUA_CLIENT_SECRET=...
    python live_demo.py                        # then open http://127.0.0.1:8000

Run-batch screenshots come back as S3 keys, so frames are pulled from the
results bucket (needs AWS read access). Ctrl-C releases the lanes.

Env knobs: CUA_POOL, CUA_LANES (default 4), CUA_DEMO_PORT (8000),
CUA_TOKEN_URL, CUA_RESULTS_BUCKET. Set CUA_DEMO_SELFTEST=1 to serve
loading/placeholder frames without touching the backend (preview the UI).
"""

from __future__ import annotations

import io
import os
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import boto3
from PIL import Image, ImageDraw, ImageFont

from cua_train import TrainClient
from cua_train.models import (
    AcquireLanesRequest,
    Action,
    ActionParameters,
    BatchSubmitRequest,
    ErrorResponse,
    ReleaseLanesRequest,
    RunConfig,
)

POOL = os.environ.get("CUA_POOL", "test-pool-please-ignore")
TOKEN_URL = os.environ.get(
    "CUA_TOKEN_URL",
    "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token",
)
RESULTS_BUCKET = os.environ.get("CUA_RESULTS_BUCKET", "cua-osgym-batch-results")
N_LANES = int(os.environ.get("CUA_LANES", "4"))
PORT = int(os.environ.get("CUA_DEMO_PORT", "8000"))
SELFTEST = os.environ.get("CUA_DEMO_SELFTEST") == "1"

W, H = 1920, 1080
SCALE = 960 / W
SW, SH = round(W * SCALE), round(H * SCALE)
ANIM_DT = 0.06  # loading-frame redraw + stream push interval (~16 fps)
RED, BLUE, ORANGE, GREY, INK = (255, 90, 90), (90, 150, 255), (255, 165, 50), (120, 120, 120), (190, 205, 230)

SQUARE = [(480, 270), (1440, 270), (1440, 810), (480, 810), (480, 270)]
TRIANGLE = [(960, 170), (1440, 880), (480, 880), (960, 170)]


def _font(size):
    try:
        return ImageFont.load_default(size=size)
    except TypeError:  # lint-ignore: swallowed-exception — old Pillow without size=
        return ImageFont.load_default()


FONT, FONT_SM, FONT_BIG = _font(22), _font(16), _font(34)


def _clicks(points):
    return [
        Action(action_type="CLICK", parameters=ActionParameters.from_dict({"x": x, "y": y, "button": "left"}))
        for x, y in points
    ]


def _short(vm):
    return vm.rsplit("-", 1)[-1][:8] if vm else "?"


# ── shared latest-frame buffers (one JPEG per lane) ────────────────────────────
_frames: list[bytes | None] = [None] * N_LANES
# Each lane's current VM id (changes on RESET); tracked so shutdown releases the
# live VMs rather than the now-stale ones returned by acquire_lanes.
_cur_vms: list[str | None] = [None] * N_LANES
_lock = threading.Lock()


def set_frame(i, jpeg):
    with _lock:
        _frames[i] = jpeg


def get_frame(i):
    with _lock:
        return _frames[i]


def _jpeg(im):
    buf = io.BytesIO()
    im.convert("RGB").save(buf, "JPEG", quality=80)
    return buf.getvalue()


def banner(im, lines, accent):
    """Top metadata strip: line 0 in the accent colour, the rest dimmed."""
    d = ImageDraw.Draw(im, "RGBA")
    d.rectangle([0, 0, im.width, 12 + 28 * len(lines)], fill=(0, 0, 0, 170))
    y = 8
    for i, ln in enumerate(lines):
        d.text((12, y), ln, fill=tuple(accent) if i == 0 else (235, 235, 235), font=FONT if i == 0 else FONT_SM)
        y += 28


CYAN, MINT = (90, 165, 255), (110, 210, 160)


def loading_frame(head, secs, status="waiting for VM…", sub="", accent=CYAN):
    """A status screen with a spinner + elapsed timer. `status`/`accent`
    distinguish the wait: acquiring a fresh VM vs. a batch running / waiting
    for its results."""
    im = Image.new("RGB", (SW, SH), (15, 17, 22))
    d = ImageDraw.Draw(im)
    cx, cy, r = SW // 2, SH // 2, 40
    d.ellipse([cx - r, cy - r, cx + r, cy + r], outline=(40, 46, 58), width=6)  # track
    a = (secs * 220) % 360  # rotating head
    d.arc([cx - r, cy - r, cx + r, cy + r], a, a + 110, fill=accent, width=6)
    d.text((cx, cy + r + 36), f"{status}  {secs:0.1f}s", fill=INK, font=FONT, anchor="mm")
    banner(im, [head, sub], accent)
    return _jpeg(im)


def path_frame(base, head, sub, accent, path, upto, prior=None):
    """A real screenshot with the click path traced + metadata banner."""
    im = base.resize((SW, SH)).convert("RGB")
    d = ImageDraw.Draw(im)
    if prior:  # earlier shape on this same VM, faded
        d.line([(x * SCALE, y * SCALE) for x, y in prior], fill=GREY, width=3)
    pts = [(x * SCALE, y * SCALE) for x, y in path[: upto + 1]]
    if len(pts) >= 2:
        d.line(pts, fill=accent, width=4)
    for sx, sy in pts:
        d.ellipse([sx - 7, sy - 7, sx + 7, sy + 7], fill=accent)
    banner(im, [head, sub], accent)
    return _jpeg(im)


def reset_frame(base, head, sub):
    im = base.resize((SW, SH)).convert("RGB")
    banner(im, [head, sub], ORANGE)
    return _jpeg(im)


# ── backend helpers ────────────────────────────────────────────────────────────
def make_client():
    return TrainClient.from_key(
        token_url=TOKEN_URL,
        client_id=os.environ["CUA_CLIENT_ID"],
        client_secret=os.environ["CUA_CLIENT_SECRET"],
    )


def wait_run(client, label, stop, idx, head, timeout=300):
    """Poll for the run; meanwhile paint a loading screen with a live timer.
    Raises on ErrorResponse (e.g. 401) so the caller can refresh the client."""
    t0 = time.time()
    last_poll = 0.0
    while not stop.is_set():
        # Redraw the spinner every frame (smooth); the VM already exists, so
        # this is "running / waiting for results", not "waiting for a VM".
        set_frame(
            idx,
            loading_frame(
                head,
                time.time() - t0,
                status="running — waiting for results…",
                sub="executing steps on the VM",
                accent=MINT,
            ),
        )
        if time.time() - last_poll >= 2:
            last_poll = time.time()
            try:
                res = client.get_label_results(pool=POOL, label=label, completed_only=True)
            except Exception as e:  # transient network blip — keep waiting
                print(f"[lane {idx}] poll {label}: {e!r}", flush=True)
            else:
                if isinstance(res, ErrorResponse):
                    raise RuntimeError(f"results error: {res}")
                runs = [r for b in res.batches for r in b.runs] if getattr(res, "batches", None) else []
                if getattr(res, "all_completed", False) and runs:
                    return runs[0]
                if time.time() - t0 > timeout:
                    return None
        stop.wait(ANIM_DT)
    return None


def run_batch(client, idx, vm, steps, stop, head):
    label = f"demo-{idx}-{uuid.uuid4().hex[:6]}"
    resp = client.submit_label_batch(
        pool=POOL,
        label=label,
        body=BatchSubmitRequest(runs=[RunConfig(vm_id=vm, steps=steps, timeout=300)], concurrency=1, keep_warm=True),
    )
    if isinstance(resp, ErrorResponse):
        raise RuntimeError(f"submit error: {resp}")
    return wait_run(client, label, stop, idx, head)


def play(idx, run, s3, stop, frame_fn):
    """Stream a run's screenshots (S3 keys) into the lane buffer, one per step."""
    shots = run.screenshots or []
    for j, k in enumerate(shots):
        if stop.is_set():
            return
        if not k:
            continue
        raw = s3.get_object(Bucket=RESULTS_BUCKET, Key=k)["Body"].read()
        set_frame(idx, frame_fn(Image.open(io.BytesIO(raw)), j, len(shots)))
        stop.wait(0.45)


# ── per-lane worker: loop square (b1) → triangle (b2) → RESET (b3), forever ─────
def lane_worker(idx, vm0, stop):
    s3 = boto3.client("s3")
    client = make_client()  # TrainClient refreshes its token transparently
    cur, it = vm0, 0
    _cur_vms[idx] = vm0
    while not stop.is_set():
        try:
            it += 1

            # batch 1 — square on the current warm VM
            r = run_batch(client, idx, cur, _clicks(SQUARE), stop, f"LANE {idx} · iter {it} · SQUARE  (batch 1)")
            if r is None:
                continue
            cur = r.vm_id
            _cur_vms[idx] = cur
            play(
                idx,
                r,
                s3,
                stop,
                lambda im, j, n: path_frame(
                    im, f"LANE {idx}   iter {it}   SQUARE {j + 1}/{n}", f"batch 1 · vm …{_short(cur)}", RED, SQUARE, j
                ),
            )

            # batch 2 — triangle on the SAME VM (reuse across batches)
            r = run_batch(client, idx, cur, _clicks(TRIANGLE), stop, f"LANE {idx} · iter {it} · TRIANGLE  (batch 2)")
            if r is None:
                continue
            cur = r.vm_id
            _cur_vms[idx] = cur
            play(
                idx,
                r,
                s3,
                stop,
                lambda im, j, n: path_frame(
                    im,
                    f"LANE {idx}   iter {it}   TRIANGLE {j + 1}/{n}",
                    f"batch 2 · vm …{_short(cur)}  (same VM, reused)",
                    BLUE,
                    TRIANGLE,
                    j,
                    prior=SQUARE,
                ),
            )

            # batch 3 — in-band RESET: hard reset → new VM
            old = cur
            r = run_batch(
                client, idx, cur, [Action(action_type="RESET")], stop, f"LANE {idx} · iter {it} · RESET  (batch 3)"
            )
            if r is None:
                continue
            cur = r.vm_id
            _cur_vms[idx] = cur
            play(
                idx,
                r,
                s3,
                stop,
                lambda im, j, n: reset_frame(
                    im, "RESET — hard reset", f"vm …{_short(old)} → …{_short(cur)}   (new VM, same lane {idx})"
                ),
            )
        except Exception as e:  # transient network blip — log and retry the iteration
            print(f"[lane {idx}] iter {it} error: {e!r}; retrying", flush=True)
            stop.wait(2)


# ── webserver: one MJPEG stream per lane ───────────────────────────────────────
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.path == "/":
            self._page()
        elif self.path.startswith("/lane/"):
            try:
                idx = int(self.path.split("/")[2])
            except (IndexError, ValueError):  # lint-ignore: swallowed-exception — bad /lane path, handled as 404
                self.send_error(404)
                return
            self._stream(idx)
        else:
            self.send_error(404)

    def _page(self):
        cards = "".join(
            f'<figure><figcaption>lane {i}</figcaption><img src="/lane/{i}"></figure>' for i in range(N_LANES)
        )
        body = f"""<!doctype html><html><head><meta charset=utf-8>
<title>cyclops-cs — square / triangle / reset loop</title>
<style>body{{background:#111;color:#eee;font-family:system-ui;margin:0;padding:24px;text-align:center}}
.grid{{display:flex;flex-wrap:wrap;gap:18px;justify-content:center}}
figure{{margin:0}} img{{width:46vw;border:1px solid #333;border-radius:8px}}
figcaption{{margin:6px;color:#9cf}} .legend{{max-width:1000px;margin:8px auto 0;color:#bbb;line-height:1.5}}
b{{color:#eee}}</style></head>
<body><h2>cyclops-cs 0.2.0 — {N_LANES} lanes looping square → triangle → reset</h2>
<p class=legend>Each tile is one <b>lane</b> = a persistent VM. Per loop it runs three batches on
the <b>same warm VM</b>: <span style="color:#ff5a5a">square</span> (batch 1) →
<span style="color:#5a96ff">triangle</span> (batch 2, <b>VM reused</b>) →
<span style="color:#ffa532">RESET</span> (batch 3, <b>new VM</b> — watch the <code>vm</code> id change).
Then it repeats.</p>
<div class=grid>{cards}</div></body></html>""".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _stream(self, idx):
        self.send_response(200)
        self.send_header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        try:
            while True:
                frame = get_frame(idx) or loading_frame("starting…", 0.0)
                self.wfile.write(b"--frame\r\nContent-Type: image/jpeg\r\n")
                self.wfile.write(f"Content-Length: {len(frame)}\r\n\r\n".encode())
                self.wfile.write(frame)
                self.wfile.write(b"\r\n")
                time.sleep(ANIM_DT)
        except (BrokenPipeError, ConnectionResetError):  # lint-ignore: swallowed-exception — viewer closed stream
            pass


def _acquire_with_timer(client, stop_anim):
    """Acquire N lanes while painting a loading+timer screen on every tile."""
    t0 = time.time()
    box = {}

    def anim():
        while not stop_anim.is_set():
            for i in range(N_LANES):
                set_frame(
                    i,
                    loading_frame(
                        f"LANE {i} · acquiring",
                        time.time() - t0,
                        status="waiting for VM…",
                        sub="acquire_lanes — provisioning a warm VM from the pool",
                    ),
                )
            time.sleep(ANIM_DT)

    t = threading.Thread(target=anim, daemon=True)
    t.start()
    try:
        # The pool may not have N warm VMs yet (503 / ErrorResponse). Keep the
        # "waiting for VM…" screen up and retry instead of crashing.
        while not stop_anim.is_set():
            res = client.acquire_lanes(pool=POOL, body=AcquireLanesRequest(n=N_LANES))
            if isinstance(res, ErrorResponse):
                print(f"acquire_lanes: {res}; retrying…", flush=True)
                time.sleep(3)
                continue
            box["lanes"] = res.vm_ids
            break
    finally:
        stop_anim.set()
        t.join(timeout=2)
    return box["lanes"]


def main():
    for i in range(N_LANES):
        set_frame(i, loading_frame("starting…", 0.0))

    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    print(f"streaming on http://127.0.0.1:{PORT}  ({N_LANES} lanes)")

    if SELFTEST:
        print("SELFTEST: serving loading frames, no backend. Ctrl-C to stop.")
        t0 = time.time()
        try:
            while True:
                for i in range(N_LANES):
                    state = (
                        ("running — waiting for results…", "executing steps on the VM", MINT)
                        if (int(t0) + i) % 2
                        else ("waiting for VM…", "acquire_lanes — provisioning a warm VM", CYAN)
                    )
                    set_frame(
                        i,
                        loading_frame(
                            f"LANE {i} · selftest", time.time() - t0, status=state[0], sub=state[1], accent=state[2]
                        ),
                    )
                time.sleep(ANIM_DT)
        except KeyboardInterrupt:  # lint-ignore: swallowed-exception — Ctrl-C stops selftest
            return

    client = make_client()
    print(f"acquire_lanes(n={N_LANES}) …")
    lanes = _acquire_with_timer(client, threading.Event())

    stop = threading.Event()
    workers = [threading.Thread(target=lane_worker, args=(i, lanes[i], stop), daemon=True) for i in range(N_LANES)]
    for w in workers:
        w.start()
    try:
        while any(w.is_alive() for w in workers):
            time.sleep(1)
    except KeyboardInterrupt:  # lint-ignore: swallowed-exception — Ctrl-C, releases in finally
        print("\nstopping …")
    finally:
        stop.set()
        for w in workers:
            w.join(timeout=10)
        # Release the VMs the lanes currently hold (RESET changes the id each
        # iteration), plus the originals, so nothing leaks back to the pool.
        to_release = sorted({*lanes, *(v for v in _cur_vms if v)})
        client.release_lanes(pool=POOL, body=ReleaseLanesRequest(vm_ids=to_release))
        print(f"released {len(to_release)} VMs; bye.")


if __name__ == "__main__":
    main()
