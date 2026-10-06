"""Synthetic fixture support for the optional SDK example. Never creates a virtual display.

ARC_EVAL_SOURCE=/tmp/arc-cua-eval-20261005 /tmp/arc-cua-eval-venv/bin/python run.py
Input is delivered only through each driver's public MCP tools. State/fault
injection belongs to the synthetic fixture, independently of either driver.
"""
import json
from contextlib import ExitStack
from datetime import datetime, timezone
import os
import platform
import select
import statistics
import subprocess
import sys
import tempfile
import tomllib
import threading
import time
from pathlib import Path

import AppKit
import Quartz

HERE = Path(__file__).resolve().parent


def wait_for(fn, timeout=5):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        value = fn()
        if value:
            return value
        time.sleep(.01)
    raise TimeoutError('independent fixture readiness/effect timeout')


def front():
    return int(AppKit.NSWorkspace.sharedWorkspace().frontmostApplication().processIdentifier())


def cursor():
    p = Quartz.CGEventGetLocation(Quartz.CGEventCreate(None))
    return (round(p.x, 1), round(p.y, 1))


def sanitize_result(content):
    """Exclude global app menus (which can contain the user's recent documents)."""
    if not isinstance(content, dict):
        return content
    out = {k:v for k,v in content.items() if k not in ('tree_markdown', 'screenshot_base64')}
    if 'elements' in out:
        excluded = set()
        clean = []
        for element in out['elements']:
            if element.get('role') == 'AXMenuBar' or element.get('parent_index') in excluded:
                excluded.add(element.get('element_index'))
            else:
                clean.append(element)
        out['elements'] = clean
    if isinstance(out.get('fresh'), dict):
        out['fresh'] = sanitize_result(out['fresh'])
    return out


class MCP:
    def __init__(self, argv, name):
        self.name = name
        self.calls = []
        self.id = 0
        self.proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=open(f'/tmp/arc-eval-{name}.stderr', 'w'), bufsize=0)
        self.buffer = b''
        try:
            initialized = self.request('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                                                      'clientInfo': {'name': 'oh-arc-eval', 'version': '1'}})
            self.server_info = initialized['serverInfo']
            self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
            self.schemas = {t['name']: t['inputSchema'] for t in self.request('tools/list', {})['tools']}
        except BaseException:
            self.close()
            raise

    def send(self, obj):
        self.proc.stdin.write((json.dumps(obj) + '\n').encode())
        self.proc.stdin.flush()

    def request(self, method, params):
        self.id += 1
        self.send({'jsonrpc': '2.0', 'id': self.id, 'method': method, 'params': params})
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if b'\n' not in self.buffer:
                if not select.select([self.proc.stdout], [], [], max(0, deadline-time.monotonic()))[0]:
                    break
                part = os.read(self.proc.stdout.fileno(), 65536)
                if not part:
                    raise RuntimeError('MCP stream ended')
                self.buffer += part
                continue
            line, self.buffer = self.buffer.split(b'\n', 1)
            obj = json.loads(line)
            if obj.get('id') == self.id:
                if 'error' in obj:
                    raise RuntimeError(str(obj['error']))
                return obj['result']
        raise TimeoutError('MCP request timed out; no input retry')

    def call(self, name, **args):
        assert name in self.schemas
        started = time.perf_counter()
        result = self.request('tools/call', {'name': name, 'arguments': args})
        ms = (time.perf_counter() - started) * 1000
        content = result.get('structuredContent')
        if content is None:
            texts = [part.get('text', '') for part in result.get('content', []) if part.get('type') == 'text']
            try:
                content = json.loads('\n'.join(texts))
            except ValueError:
                content = {'text': '\n'.join(texts)}
        # Retain timing only. Nested observations may contain private app menus.
        self.calls.append({'tool': name, 'ms': ms})
        return content, bool(result.get('isError'))

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.terminate()
            self.proc.wait(timeout=5)


class Fixture:
    def __init__(self, start='shown'):
        self.temp = tempfile.TemporaryDirectory(prefix='oh-arc-fixture-')
        self.path = Path(self.temp.name) / 'state.json'
        self.log = open(Path(self.temp.name) / 'stderr', 'w')
        self.proc = subprocess.Popen([sys.executable, str(HERE/'fixture.py'), str(self.path), start],
                                     stdout=subprocess.DEVNULL, stderr=self.log)
        try:
            wait_for(lambda: self.path.exists(), 8)
            if start != 'shown':
                wait_for(lambda: self.state().get(start))
        except BaseException:
            state = self.state()
            self.close()
            raise RuntimeError(f'fixture setup failed: {state}')
        self.pid = self.proc.pid
        self.command = 0

    def state(self):
        try:
            return json.loads(self.path.read_text())
        except (OSError, ValueError):
            return {}

    def mutate(self, action):
        self.command += 1
        self.path.with_suffix('.command').write_text(json.dumps({'action': action, 'id': self.command}))
        wait_for(lambda: self.state().get('applied') == self.command)
        # Give structural notifications time to arrive; no driver observation here.
        time.sleep(.1)

    def close(self):
        self.proc.terminate()
        self.proc.wait(timeout=5)
        self.log.close()
        self.temp.cleanup()


def windows(pid):
    options = Quartz.kCGWindowListOptionAll | Quartz.kCGWindowListExcludeDesktopElements
    found = Quartz.CGWindowListCopyWindowInfo(options, Quartz.kCGNullWindowID) or []
    return {str(w.get('kCGWindowName', '')): int(w['kCGWindowNumber']) for w in found
            if int(w.get('kCGWindowOwnerPID', 0)) == pid and int(w.get('kCGWindowLayer', 0)) == 0}

