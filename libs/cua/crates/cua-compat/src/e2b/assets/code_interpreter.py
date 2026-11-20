#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Code-interpreter server for the Cua E2B gateway (port 49999).

Speaks the HTTP API the e2b-code-interpreter SDKs use against E2B's
`code-interpreter-v1` template: POST/GET /contexts, DELETE /contexts/{id},
POST /contexts/{id}/restart, POST /execute (a stream of JSON lines). Standard
library only. Each context is one persistent worker process, so variables,
imports and functions survive between calls. Languages: python and bash.
"""
import ast
import json
import os
import signal
import subprocess
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(os.environ.get("CUA_CODE_INTERPRETER_PORT", "49999"))
LANGS = ("python", "bash")

WORKER = r'''
import ast, io, json, os, select, sys, threading, time, traceback
ctl = os.fdopen(os.dup(1), "w", buffering=1)
cl = threading.Lock()
def emit(o):
    with cl:
        ctl.write(json.dumps(o) + "\n"); ctl.flush()
SENT = b"\0CUA-DONE\0"
done = {1: threading.Event(), 2: threading.Event()}
pl = threading.Lock()
bufs = {1: b"", 2: b""}
rds = {}
def step(fd, kind):
    """Reads what the fd pipe holds and emits it (the caller holds `pl`).
    False at end of file."""
    d = os.read(rds[fd], 65536)
    if not d:
        return False
    buf = bufs[fd] + d
    while True:
        i = buf.find(SENT)
        if i < 0:
            break
        if i:
            send(kind, buf[:i])
        buf = buf[i + len(SENT):]
        done[fd].set()
    keep = 0
    for k in range(1, len(SENT)):
        if buf.endswith(SENT[:k]):
            keep = k
    cut = len(buf) - keep
    if cut:
        send(kind, buf[:cut])
        buf = buf[cut:]
    bufs[fd] = buf
    return True
def pump(fd, kind):
    while True:
        select.select([rds[fd]], [], [])
        with pl:
            if select.select([rds[fd]], [], [], 0)[0] and not step(fd, kind):
                return
def drain():
    """Emits what children already wrote to the fd pipes, so it comes before
    the Python text written after it (the caller holds `pl`)."""
    for fd, kind in ((1, "stdout"), (2, "stderr")):
        while select.select([rds[fd]], [], [], 0)[0]:
            if not step(fd, kind):
                break
def send(kind, b):
    emit({"type": kind, "text": b.decode("utf-8", "replace"), "timestamp": time.time_ns()})
for fd, kind in ((1, "stdout"), (2, "stderr")):
    r, w = os.pipe()
    os.dup2(w, fd)
    os.close(w)
    rds[fd] = r
    threading.Thread(target=pump, args=(fd, kind), daemon=True).start()
ol = threading.Lock()
class Out(io.TextIOBase):
    # Python-level writes are emitted in order, batched for 50 ms as Jupyter
    # does ("a\nb\n" from two prints is one message); the fd pipes above
    # carry what child processes write.
    def __init__(self, kind):
        self.kind = kind
        self.buf = ""
    def writable(self):
        return True
    def write(self, s):
        with ol:
            self.buf += s
        return len(s)
    def flush(self):
        with pl:
            drain()
            with ol:
                text, self.buf = self.buf, ""
            if text:
                emit({"type": self.kind, "text": text, "timestamp": time.time_ns()})
    def fileno(self):
        return 1 if self.kind == "stdout" else 2
sys.stdout = Out("stdout")
sys.stderr = Out("stderr")
def ticker():
    while True:
        time.sleep(0.05)
        sys.stdout.flush(); sys.stderr.flush()
threading.Thread(target=ticker, daemon=True).start()
sys.stdin = open(os.devnull)
g = {"__name__": "__main__"}
count = 0
def mime(v):
    r = {}
    for key, meth in (("html", "_repr_html_"), ("markdown", "_repr_markdown_"), ("svg", "_repr_svg_"),
                      ("png", "_repr_png_"), ("jpeg", "_repr_jpeg_"), ("latex", "_repr_latex_"),
                      ("javascript", "_repr_javascript_")):
        f = getattr(type(v), meth, None)
        if f is None:
            continue
        try:
            x = f(v)
        except Exception:
            continue
        if isinstance(x, bytes):
            import base64
            x = base64.b64encode(x).decode()
        if isinstance(x, str):
            r[key] = x
    return r
for line in os.fdopen(0, "r", encoding="utf-8"):
    req = json.loads(line)
    count += 1
    saved = {}
    for k, v in (req.get("env") or {}).items():
        saved[k] = os.environ.get(k)
        os.environ[k] = v
    try:
        os.chdir(req["cwd"])
    except Exception:
        pass
    code = req["code"]
    try:
        tree = ast.parse(code, "<cell>", "exec")
        last = None
        if tree.body and isinstance(tree.body[-1], ast.Expr) and not code.rstrip().endswith(";"):
            last = ast.Expression(tree.body.pop().value)
        exec(compile(tree, "<cell>", "exec"), g)
        if last is not None:
            v = eval(compile(last, "<cell>", "eval"), g)
            if v is not None:
                emit({"type": "result", "text": v if isinstance(v, str) else repr(v), "is_main_result": True, **mime(v)})
    except BaseException as ex:
        tb = traceback.format_exception(type(ex), ex, ex.__traceback__)
        if isinstance(ex, SyntaxError):
            tb = traceback.format_exception_only(type(ex), ex)
        else:
            tb = [x for x in tb if 'File "<string>"' not in x]
        emit({"type": "error", "name": type(ex).__name__, "value": str(ex), "traceback": "".join(tb)})
    try:
        sys.stdout.flush(); sys.stderr.flush()
    except Exception:
        pass
    for e in done.values():
        e.clear()
    os.write(1, SENT); os.write(2, SENT)
    for e in done.values():
        e.wait(10)
    for k, v in saved.items():
        if v is None:
            os.environ.pop(k, None)
        else:
            os.environ[k] = v
    emit({"type": "number_of_executions", "execution_count": count})
    emit({"type": "end_of_execution"})
'''


class Context:
    def __init__(self, cid, language, cwd):
        self.id, self.language, self.cwd = cid, language, cwd
        self.lock = threading.Lock()
        self.proc = None
        self.count = 0

    def json(self):
        return {"id": self.id, "language": self.language, "cwd": self.cwd}

    def worker(self):
        if self.proc is None or self.proc.poll() is not None:
            self.proc = subprocess.Popen(
                [sys.executable, "-u", "-c", WORKER],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                cwd=self.cwd if os.path.isdir(self.cwd) else None, text=True, bufsize=1,
            )
        return self.proc

    def stop(self):
        p, self.proc = self.proc, None
        if p and p.poll() is None:
            p.kill()
            p.wait()

    def run(self, code, env, emit):
        """Runs `code`, calling emit(dict) for each event. Returns False if the client went away."""
        with self.lock:
            if self.language == "bash":
                return self.run_bash(code, env, emit)
            p = self.worker()
            p.stdin.write(json.dumps({"code": code, "env": env or {}, "cwd": self.cwd}) + "\n")
            p.stdin.flush()
            while True:
                line = p.stdout.readline()
                if not line:
                    emit({"type": "error", "name": "KernelDied", "value": "the interpreter process exited", "traceback": ""})
                    self.proc = None
                    return True
                ev = json.loads(line)
                if ev["type"] == "end_of_execution":
                    return True
                try:
                    emit(ev)
                except OSError:
                    p.send_signal(signal.SIGINT)
                    emit = lambda _e: None

    def run_bash(self, code, env, emit):
        e = dict(os.environ)
        e.update(env or {})
        p = subprocess.Popen(["bash", "-c", code], cwd=self.cwd if os.path.isdir(self.cwd) else None, env=e,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, stdin=subprocess.DEVNULL)
        lock = threading.Lock()
        broken = []

        def pump(f, kind):
            for chunk in iter(lambda: f.readline(), b""):
                with lock:
                    try:
                        emit({"type": kind, "text": chunk.decode("utf-8", "replace"), "timestamp": time.time_ns()})
                    except OSError:
                        broken.append(1)
                        p.kill()
        ts = [threading.Thread(target=pump, args=(p.stdout, "stdout")), threading.Thread(target=pump, args=(p.stderr, "stderr"))]
        for t in ts:
            t.start()
        rc = p.wait()
        for t in ts:
            t.join()
        self.count += 1
        if rc != 0 and not broken:
            emit({"type": "error", "name": "CommandExitError", "value": f"exit status {rc}", "traceback": ""})
        emit({"type": "number_of_executions", "execution_count": self.count})
        return True


HOME = os.path.expanduser("~")
CONTEXTS = {}
DEFAULTS = {}
LOCK = threading.Lock()


def default_ctx(lang):
    with LOCK:
        c = DEFAULTS.get(lang)
        if c is None:
            c = DEFAULTS[lang] = Context("default-" + lang, lang, HOME)
            CONTEXTS[c.id] = c
        return c


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "cua-code-interpreter"

    def log_message(self, *a):
        pass

    def body(self):
        n = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(n) if n else b""
        return json.loads(raw) if raw.strip() else {}

    def send_json(self, status, obj):
        b = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def err(self, status, msg):
        b = msg.encode()
        self.send_response(status)
        self.send_header("content-type", "text/plain; charset=utf-8")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def route(self):
        path = self.path.split("?")[0].rstrip("/")
        parts = [p for p in path.split("/") if p]
        m = self.command
        try:
            if parts == ["health"] and m == "GET":
                return self.send_json(200, {"status": "ok"})
            if parts == ["contexts"] and m == "POST":
                b = self.body()
                lang = b.get("language") or "python"
                if lang not in LANGS:
                    return self.err(400, f"language {lang} is not available in this code interpreter; available: {', '.join(LANGS)}")
                cwd = b.get("cwd") or HOME
                c = Context(str(uuid.uuid4()), lang, cwd)
                with LOCK:
                    CONTEXTS[c.id] = c
                return self.send_json(200, c.json())
            if parts == ["contexts"] and m == "GET":
                default_ctx("python")
                with LOCK:
                    return self.send_json(200, [c.json() for c in CONTEXTS.values()])
            if len(parts) >= 2 and parts[0] == "contexts":
                c = CONTEXTS.get(parts[1])
                if c is None:
                    return self.err(404, f"Context {parts[1]} not found")
                if len(parts) == 2 and m == "DELETE":
                    c.stop()
                    with LOCK:
                        CONTEXTS.pop(c.id, None)
                        for k, v in list(DEFAULTS.items()):
                            if v is c:
                                DEFAULTS.pop(k)
                    return self.send_json(200, {})
                if len(parts) == 3 and parts[2] == "restart" and m == "POST":
                    c.stop()
                    return self.send_json(200, {})
            if parts == ["execute"] and m == "POST":
                return self.execute()
            return self.err(404, "404 page not found")
        except json.JSONDecodeError as e:
            return self.err(400, f"invalid request body: {e}")

    def execute(self):
        b = self.body()
        code = b.get("code")
        if code is None:
            return self.err(400, "code is required")
        cid, lang = b.get("context_id"), b.get("language")
        if cid and lang:
            return self.err(400, "context_id and language are mutually exclusive")
        if cid:
            c = CONTEXTS.get(cid)
            if c is None:
                return self.err(404, f"Context {cid} not found")
        else:
            lang = lang or "python"
            if lang not in LANGS:
                return self.err(400, f"language {lang} is not available in this code interpreter; available: {', '.join(LANGS)}")
            c = default_ctx(lang)
        self.send_response(200)
        self.send_header("content-type", "application/x-ndjson")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()

        def emit(ev):
            data = (json.dumps(ev) + "\n").encode()
            self.wfile.write(b"%x\r\n" % len(data) + data + b"\r\n")
            self.wfile.flush()
        try:
            c.run(code, b.get("env_vars"), emit)
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()
        except OSError:
            self.close_connection = True

    do_GET = do_POST = do_DELETE = route


def main():
    ThreadingHTTPServer.daemon_threads = True
    ThreadingHTTPServer.request_queue_size = 64
    ThreadingHTTPServer(("0.0.0.0", PORT), H).serve_forever()


if __name__ == "__main__":
    main()
