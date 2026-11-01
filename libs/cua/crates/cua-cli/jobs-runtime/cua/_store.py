"""Shared, attempt-fenced store for a run's data_dir (standard library only).

Internal to the Cua jobs runtime: `_cua_entry` restores data_dir from it before
user code and rank 0 syncs data_dir into it in the background.  User code
never calls it.
"""
from __future__ import annotations

import configparser
import contextlib
import datetime as _dt
import fcntl
import hashlib
import hmac
import json
import os
import random
import shutil
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Any

# --------------------------------------------------------------------------
# Layout and rules
# --------------------------------------------------------------------------
# A run's snapshots of data_dir live under ``<url>/<run_id>/``::
#
#     attempt-<a>/step-<n>/files/<rel path>   files first written by snapshot n of attempt a
#     attempt-<a>/step-<n>/manifest.json      every file of snapshot n: sha256, size, and the
#                                             snapshot dir ("src") that holds its bytes
#     COMMITTED/step-<n>                      commit marker, created once (create-only)
#     LATEST                                  {"step", "attempt", "path"}; moves only forward
#
# "step" is the snapshot's sequence number in the run (1, 2, 3, ...), not a
# training step.  A file that did not change since the previous snapshot is not
# uploaded again: the new manifest points at the snapshot that holds it.
#
# Rules that make a re-placed or cut-off attempt safe:
#
# * ``LATEST`` changes only by compare-and-swap (S3 ``If-Match: <etag>`` /
#   ``If-None-Match: *``; file: an exclusive lock plus an atomic rename), and
#   only forward: its attempt never decreases and its step never decreases.
# * A new attempt *claims* the run (``LATEST.attempt = a``) before user code
#   runs, so an older attempt that is still running somewhere can no longer
#   commit: its commit sees ``LATEST.attempt > a`` and is rejected
#   (``StaleAttempt``), and if it raced the claim the CAS fails and it re-checks.
# * A committed step is never overwritten: the marker is create-only, and the
#   files of attempt a sit in attempt a's own directory.
# * GC keeps the newest ``keep`` committed snapshots (and every object their
#   manifests point at) and deletes what older attempts uploaded but never
#   committed.  Markers stay (they are tiny) so a step number can never be
#   committed twice.
#
# Backends: ``file:///path`` (a shared mount: NAS, NFS, a synced folder) and
# ``s3://bucket/prefix``.  S3 is spoken directly over HTTPS with SigV4 rather
# than through boto3 so that the runtime keeps working in any job image without
# extra packages; conditional writes are two plain headers.  Credentials come
# from ``AWS_ACCESS_KEY_ID``/``AWS_SECRET_ACCESS_KEY``[/``AWS_SESSION_TOKEN``],
# then the shared credentials file (``AWS_PROFILE``), then the EC2 instance role
# (IMDSv2).  ``AWS_ENDPOINT_URL_S3``/``AWS_ENDPOINT_URL`` point it at an
# S3-compatible service.
MANIFEST = "manifest.json"
FILES = "files"


class StoreError(RuntimeError):
    """The checkpoint store failed (after retries)."""


class StaleAttempt(StoreError):
    """This attempt was superseded; it must not commit checkpoints."""


class _Conflict(Exception):
    """A conditional write lost (412 / exists / etag changed)."""


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


# --------------------------------------------------------------------------
# Backends: a tiny object-store interface
#   get(key) -> (bytes, etag) | None
#   put(key, data_or_path, if_none_match=False, if_match=None)  raises _Conflict
#   list(prefix) -> [key]       delete(key)
# --------------------------------------------------------------------------
_THREAD_LOCKS: dict[str, threading.Lock] = {}
_THREAD_LOCKS_GUARD = threading.Lock()


@contextlib.contextmanager
def _exclusive(lock_path: Path):
    """Exclusive lock across processes (POSIX lockf, NFSv4-safe) and threads
    (POSIX record locks are per process, so threads also take a local lock)."""
    with _THREAD_LOCKS_GUARD:
        tl = _THREAD_LOCKS.setdefault(str(lock_path), threading.Lock())
    with tl:
        fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o644)
        try:
            fcntl.lockf(fd, fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.lockf(fd, fcntl.LOCK_UN)
        finally:
            os.close(fd)


def _tmp_name(p: Path) -> Path:
    return p.with_name(f".{p.name}.{os.getpid()}.{threading.get_ident()}.{random.getrandbits(32):08x}.tmp")


class FileBackend:
    def __init__(self, root: str):
        self.root = Path(root)
        self.root.mkdir(parents=True, exist_ok=True)

    def _p(self, key: str) -> Path:
        parts = key.split("/")
        if any(p in ("", ".", "..") for p in parts):
            raise ValueError(f"bad key {key!r}")
        return self.root.joinpath(*parts)

    def get(self, key: str):
        p = self._p(key)
        try:
            with open(p, "rb") as f:  # content and etag from the same inode
                st = os.fstat(f.fileno())
                data = f.read()
        except (FileNotFoundError, NotADirectoryError):
            return None
        return data, f"{st.st_ino}-{st.st_mtime_ns}-{st.st_size}"

    def get_to(self, key: str, dst: Path) -> bool:
        src = self._p(key)
        if not src.is_file():
            return False
        shutil.copyfile(src, dst)
        return True

    def put(self, key: str, data, if_none_match: bool = False, if_match: str | None = None) -> None:
        for i in range(5):  # GC may prune an empty parent dir between mkdir and write
            try:
                return self._put(key, data, if_none_match, if_match)
            except FileNotFoundError:
                if i == 4:
                    raise

    def _put(self, key: str, data, if_none_match: bool, if_match: str | None) -> None:
        p = self._p(key)
        p.parent.mkdir(parents=True, exist_ok=True)
        if if_none_match:
            # Create-only: O_CREAT|O_EXCL decides the winner, then the content is written.
            try:
                fd = os.open(p, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
            except FileExistsError:
                raise _Conflict(key) from None
            with os.fdopen(fd, "wb") as f:
                f.write(data if isinstance(data, (bytes, bytearray)) else Path(data).read_bytes())
                f.flush()
                os.fsync(f.fileno())
            return
        tmp = _tmp_name(p)
        if isinstance(data, (bytes, bytearray)):
            tmp.write_bytes(bytes(data))
        else:
            shutil.copyfile(data, tmp)
        with open(tmp, "rb") as f:
            os.fsync(f.fileno())
        if if_match is None:
            os.replace(tmp, p)
            return
        # Compare-and-swap under an exclusive lock.
        with _exclusive(p.with_name(f".{p.name}.lock")):
            cur = self.get(key)
            if cur is None or cur[1] != if_match:
                tmp.unlink(missing_ok=True)
                raise _Conflict(key)
            os.replace(tmp, p)

    def put_cas_new(self, key: str, data: bytes) -> None:
        """Create `key` only if absent, atomically visible (for LATEST)."""
        p = self._p(key)
        p.parent.mkdir(parents=True, exist_ok=True)
        with _exclusive(p.with_name(f".{p.name}.lock")):
            if p.exists():
                raise _Conflict(key)
            tmp = _tmp_name(p)
            tmp.write_bytes(data)
            with open(tmp, "rb") as f:
                os.fsync(f.fileno())
            os.replace(tmp, p)

    def list(self, prefix: str) -> list[str]:
        base = self._p(prefix.rstrip("/")) if prefix.strip("/") else self.root
        if not base.exists():
            return []
        out = []
        for dirpath, _dirs, files in os.walk(base):
            for name in files:
                if name.startswith("."):
                    continue
                full = Path(dirpath) / name
                out.append(full.relative_to(self.root).as_posix())
        return sorted(out)

    def delete(self, key: str) -> None:
        p = self._p(key)
        try:
            p.unlink()
        except FileNotFoundError:
            pass
        # prune empty dirs up to the root
        d = p.parent
        while d != self.root and d.is_dir():
            try:
                d.rmdir()
            except OSError:
                break
            d = d.parent


class _Creds:
    def __init__(self, key: str, secret: str, token: str | None, expires: float | None = None):
        self.key, self.secret, self.token, self.expires = key, secret, token, expires


def _imds_creds(timeout: float = 2.0) -> _Creds | None:
    base = "http://169.254.169.254"
    try:
        req = urllib.request.Request(base + "/latest/api/token", method="PUT",
                                     headers={"X-aws-ec2-metadata-token-ttl-seconds": "21600"})
        token = urllib.request.urlopen(req, timeout=timeout).read().decode()
        hdr = {"X-aws-ec2-metadata-token": token}
        url = base + "/latest/meta-data/iam/security-credentials/"
        role = urllib.request.urlopen(urllib.request.Request(url, headers=hdr), timeout=timeout).read().decode()
        role = role.strip().splitlines()[0]
        doc = json.loads(urllib.request.urlopen(urllib.request.Request(url + role, headers=hdr),
                                                timeout=timeout).read())
        exp = _dt.datetime.strptime(doc["Expiration"], "%Y-%m-%dT%H:%M:%SZ").replace(
            tzinfo=_dt.timezone.utc).timestamp()
        return _Creds(doc["AccessKeyId"], doc["SecretAccessKey"], doc.get("Token"), exp)
    except Exception:
        return None


def _load_creds(env) -> _Creds:
    if env.get("AWS_ACCESS_KEY_ID") and env.get("AWS_SECRET_ACCESS_KEY"):
        return _Creds(env["AWS_ACCESS_KEY_ID"], env["AWS_SECRET_ACCESS_KEY"], env.get("AWS_SESSION_TOKEN") or None)
    path = env.get("AWS_SHARED_CREDENTIALS_FILE") or os.path.expanduser("~/.aws/credentials")
    profile = env.get("AWS_PROFILE") or "default"
    if os.path.isfile(path):
        cp = configparser.ConfigParser()
        cp.read(path)
        if cp.has_section(profile) and cp.has_option(profile, "aws_access_key_id"):
            s = cp[profile]
            return _Creds(s["aws_access_key_id"], s["aws_secret_access_key"], s.get("aws_session_token"))
    c = _imds_creds()
    if c:
        return c
    raise StoreError("no AWS credentials for the S3 checkpoint store: pass AWS_ACCESS_KEY_ID/"
                     "AWS_SECRET_ACCESS_KEY (cua.Secret.from_local_env) or run on a host with an instance role")


def _canon_query(query: dict[str, str]) -> str:
    q = urllib.parse.quote
    return "&".join(f"{q(k, safe='-_.~')}={q(v, safe='-_.~')}" for k, v in sorted(query.items()))


class S3Backend:
    """S3 over HTTPS + SigV4 (path-style requests; virtual-host not needed)."""

    RETRIES = 6

    def __init__(self, bucket: str, prefix: str, env=None):
        self.env = os.environ if env is None else env
        self.bucket = bucket
        self.prefix = prefix.strip("/")
        self.region = self.env.get("AWS_REGION") or self.env.get("AWS_DEFAULT_REGION") or "us-east-1"
        ep = self.env.get("AWS_ENDPOINT_URL_S3") or self.env.get("AWS_ENDPOINT_URL")
        self.endpoint = (ep or f"https://s3.{self.region}.amazonaws.com").rstrip("/")
        self._creds: _Creds | None = None

    def creds(self) -> _Creds:
        c = self._creds
        if c is None or (c.expires and c.expires - time.time() < 300):
            c = self._creds = _load_creds(self.env)
        return c

    def _key(self, key: str) -> str:
        return f"{self.prefix}/{key}" if self.prefix else key

    # ---- SigV4 -----------------------------------------------------------
    def _sign(self, method: str, path: str, query: dict[str, str], headers: dict[str, str],
              payload_hash: str, now: _dt.datetime | None = None) -> dict[str, str]:
        c = self.creds()
        now = now or _dt.datetime.now(_dt.timezone.utc)
        amz_date = now.strftime("%Y%m%dT%H%M%SZ")
        day = now.strftime("%Y%m%d")
        host = urllib.parse.urlsplit(self.endpoint).netloc
        h = {k.lower(): str(v).strip() for k, v in headers.items()}
        h["host"] = host
        h["x-amz-date"] = amz_date
        h["x-amz-content-sha256"] = payload_hash
        if c.token:
            h["x-amz-security-token"] = c.token
        names = sorted(h)
        canon_headers = "".join(f"{n}:{h[n]}\n" for n in names)
        signed = ";".join(names)
        canon_query = _canon_query(query)
        canon = "\n".join([method, urllib.parse.quote(path, safe="/-_.~"), canon_query,
                           canon_headers, signed, payload_hash])
        scope = f"{day}/{self.region}/s3/aws4_request"
        to_sign = "\n".join(["AWS4-HMAC-SHA256", amz_date, scope,
                             hashlib.sha256(canon.encode()).hexdigest()])

        def hm(k: bytes, m: str) -> bytes:
            return hmac.new(k, m.encode(), hashlib.sha256).digest()

        k = hm(hm(hm(hm(("AWS4" + c.secret).encode(), day), self.region), "s3"), "aws4_request")
        sig = hmac.new(k, to_sign.encode(), hashlib.sha256).hexdigest()
        h["authorization"] = (f"AWS4-HMAC-SHA256 Credential={c.key}/{scope}, "
                              f"SignedHeaders={signed}, Signature={sig}")
        return h

    def _request(self, method: str, key: str | None, query: dict[str, str] | None = None,
                 headers: dict[str, str] | None = None, body: bytes | Path | None = None,
                 ok_missing: bool = False, sink: Path | None = None):
        """-> (status, headers, body bytes). Raises _Conflict on 412/409."""
        query = query or {}
        path = f"/{self.bucket}" + (f"/{self._key(key)}" if key is not None else "")
        if isinstance(body, Path):
            payload_hash = sha256_file(body)
            size = body.stat().st_size
        else:
            data = body or b""
            payload_hash = hashlib.sha256(data).hexdigest()
            size = len(data)
        last: Exception | None = None
        for i in range(self.RETRIES):
            hdrs = dict(headers or {})
            if method == "PUT":
                hdrs["content-length"] = str(size)
            signed = self._sign(method, path, query, hdrs, payload_hash)
            url = self.endpoint + urllib.parse.quote(path, safe="/-_.~")
            if query:
                url += "?" + _canon_query(query)
            stream = open(body, "rb") if isinstance(body, Path) else None
            req = urllib.request.Request(url, method=method,
                                         data=stream if stream else (body if method == "PUT" else None),
                                         headers=signed)
            try:
                with urllib.request.urlopen(req, timeout=120) as resp:
                    if sink is not None:
                        tmp = sink.with_name(sink.name + ".part")
                        with open(tmp, "wb") as f:
                            shutil.copyfileobj(resp, f, 1 << 20)
                        os.replace(tmp, sink)
                        return resp.status, dict(resp.headers), b""
                    return resp.status, dict(resp.headers), resp.read()
            except urllib.error.HTTPError as e:
                payload = e.read()
                e.close()
                if e.code in (409, 412):
                    raise _Conflict(f"{method} {key}: {e.code}") from None
                if e.code == 404 and ok_missing:
                    return 404, dict(e.headers), payload
                if e.code in (400, 403) and b"ExpiredToken" in payload:
                    self._creds = None
                    last = StoreError(f"S3 {method} {key}: expired token")
                elif e.code in (429, 500, 502, 503, 504):
                    last = StoreError(f"S3 {method} {key}: HTTP {e.code}")
                else:
                    raise StoreError(f"S3 {method} {key}: HTTP {e.code} {payload[:300]!r}") from None
            except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as e:
                last = StoreError(f"S3 {method} {key}: {e}")
            finally:
                if stream:
                    stream.close()
            time.sleep(min(20.0, 0.5 * 2 ** i) * random.random())
        raise last or StoreError(f"S3 {method} {key} failed")

    def get(self, key: str):
        st, h, body = self._request("GET", key, ok_missing=True)
        if st == 404:
            return None
        etag = {k.lower(): v for k, v in h.items()}.get("etag", "")
        return body, etag

    def get_to(self, key: str, dst: Path) -> bool:
        st, _h, _b = self._request("GET", key, ok_missing=True, sink=dst)
        return st != 404

    def put(self, key: str, data, if_none_match: bool = False, if_match: str | None = None) -> None:
        hdrs = {}
        if if_none_match:
            hdrs["if-none-match"] = "*"
        if if_match is not None:
            hdrs["if-match"] = if_match
        body = data if isinstance(data, (bytes, bytearray)) else Path(data)
        self._request("PUT", key, headers=hdrs, body=bytes(body) if isinstance(body, bytearray) else body)

    def put_cas_new(self, key: str, data: bytes) -> None:
        self.put(key, data, if_none_match=True)

    def list(self, prefix: str) -> list[str]:
        out: list[str] = []
        token = None
        full = self._key(prefix)
        while True:
            q = {"list-type": "2", "prefix": full}
            if token:
                q["continuation-token"] = token
            _st, _h, body = self._request("GET", None, query=q)
            root = ET.fromstring(body)
            ns = root.tag.split("}")[0] + "}" if root.tag.startswith("{") else ""
            for c in root.findall(f"{ns}Contents"):
                k = c.findtext(f"{ns}Key") or ""
                out.append(k[len(self.prefix) + 1:] if self.prefix else k)
            if (root.findtext(f"{ns}IsTruncated") or "false") != "true":
                break
            token = root.findtext(f"{ns}NextContinuationToken")
        return sorted(out)

    def delete(self, key: str) -> None:
        self._request("DELETE", key, ok_missing=True)


def open_backend(url: str, env=None):
    u = urllib.parse.urlsplit(url)
    if u.scheme == "file":
        path = urllib.parse.unquote(u.path)
        if u.netloc and u.netloc != "localhost":
            raise ValueError(f"file:// checkpoint URLs must be local paths: {url!r}")
        return FileBackend(path)
    if u.scheme == "s3":
        if not u.netloc:
            raise ValueError(f"s3 checkpoint URL needs a bucket: {url!r}")
        return S3Backend(u.netloc, u.path, env)
    raise ValueError(f"checkpoint URL must be s3://bucket/prefix or file:///path, got {url!r}")


# --------------------------------------------------------------------------
# The store: layout + fencing on top of a backend
# --------------------------------------------------------------------------
class CheckpointStore:
    def __init__(self, url: str, run_id: str, attempt: int, env=None, backend=None, keep: int = 3):
        self.url = url
        self.run_id = run_id or "local"
        if "/" in self.run_id or self.run_id in (".", ".."):
            raise ValueError(f"bad run id {self.run_id!r}")
        self.attempt = int(attempt)
        self.keep = keep
        self.b = backend or open_backend(url, env)
        self._claimed = False

    def k(self, *parts: str) -> str:
        return "/".join((self.run_id, *parts))

    @staticmethod
    def step_dir(attempt: int, step: int) -> str:
        return f"attempt-{attempt}/step-{step}"

    # ---- LATEST ----------------------------------------------------------
    def latest(self) -> dict[str, Any] | None:
        got = self.b.get(self.k("LATEST"))
        if got is None:
            return None
        try:
            doc = json.loads(got[0])
            return {"step": int(doc["step"]), "attempt": int(doc["attempt"]), "path": doc.get("path")}
        except (ValueError, KeyError, TypeError) as e:
            raise StoreError(f"corrupt LATEST in {self.url}/{self.run_id}: {e}") from None

    def _advance(self, step: int | None, path: str | None) -> dict[str, Any] | None:
        """CAS LATEST forward to (step, self.attempt, path).  step=None keeps the step (claim).

        Raises StaleAttempt when a newer attempt owns LATEST.  Returns None
        (nothing to do) when LATEST already holds this step or a newer one."""
        for _ in range(50):
            got = self.b.get(self.k("LATEST"))
            cur = None
            if got is not None:
                d = json.loads(got[0])
                cur = {"step": int(d["step"]), "attempt": int(d["attempt"]), "path": d.get("path")}
            if cur and cur["attempt"] > self.attempt:
                raise StaleAttempt(
                    f"attempt {self.attempt} is stale: attempt {cur['attempt']} owns the run "
                    f"(LATEST step {cur['step']})")
            if step is None:  # claim
                if cur and cur["attempt"] == self.attempt:
                    return cur
                new = {"step": cur["step"] if cur else 0, "attempt": self.attempt,
                       "path": cur["path"] if cur else None}
            else:
                if cur and cur["step"] >= step:
                    return None  # already covered by a committed step (ours or an older attempt's)
                new = {"step": step, "attempt": self.attempt, "path": path}
            data = (json.dumps(new, sort_keys=True) + "\n").encode()
            try:
                if got is None:
                    self.b.put_cas_new(self.k("LATEST"), data)
                else:
                    self.b.put(self.k("LATEST"), data, if_match=got[1])
                return new
            except _Conflict:
                time.sleep(random.random() * 0.2)
                continue
        raise StoreError("LATEST kept changing under us (50 CAS conflicts)")

    def claim(self) -> dict[str, Any]:
        """Fence older attempts: from now on only this attempt (or a newer one) may commit."""
        doc = self._advance(None, None)
        self._claimed = True
        return doc

    # ---- write + commit --------------------------------------------------
    def upload(self, step: int, files: dict[str, Path], world_size: int = 1,
               reuse: dict[str, dict[str, Any]] | None = None) -> str | None:
        """Upload a snapshot's new files and its manifest (not committed yet).

        `files` maps a relative path (``a/b.bin``) to a local file to upload;
        `reuse` maps a relative path to the manifest entry of an unchanged file
        in an earlier snapshot ({"sha256", "size", "src"}), which is not
        uploaded again.  None: LATEST already holds this step or a newer one."""
        cur = self.latest()
        if cur and cur["attempt"] > self.attempt:
            raise StaleAttempt(f"attempt {self.attempt} is stale: attempt {cur['attempt']} owns the run")
        if cur and cur["step"] >= step:
            return None  # already committed; nothing to upload
        sd = self.step_dir(self.attempt, step)
        manifest = {"run_id": self.run_id, "step": step, "attempt": self.attempt,
                    "world_size": world_size, "created": time.time(), "files": {}}
        for name, meta in sorted((reuse or {}).items()):
            check_rel(name)
            manifest["files"][name] = {"sha256": meta["sha256"], "size": int(meta["size"]),
                                       "src": meta["src"]}
        for name, path in sorted(files.items()):
            check_rel(name)
            manifest["files"][name] = {"sha256": sha256_file(path), "size": path.stat().st_size, "src": sd}
            self.b.put(self.k(sd, FILES, name), path)
        self.b.put(self.k(sd, MANIFEST), (json.dumps(manifest, sort_keys=True, indent=1) + "\n").encode())
        return sd

    def commit(self, step: int, path: str) -> bool:
        """Make `path` (an uploaded step dir) step `step`: CAS LATEST forward, then the marker.

        CAS first: LATEST is the linearization point, so a fenced attempt never
        leaves a marker behind.  False: a committed step >= `step` exists."""
        if not self._claimed:
            self.claim()
        if self._advance(step, path) is None:
            return False
        marker = json.dumps({"step": step, "attempt": self.attempt, "path": path}).encode()
        try:
            self.b.put(self.k("COMMITTED", f"step-{step}"), marker, if_none_match=True)
        except _Conflict:
            got = self.b.get(self.k("COMMITTED", f"step-{step}"))
            if not got or json.loads(got[0]).get("path") != path:
                raise StoreError(f"commit marker for step {step} exists for another attempt") from None
        return True

    def save(self, step: int, files: dict[str, Path], world_size: int = 1,
             reuse: dict[str, dict[str, Any]] | None = None) -> str | None:
        """Upload + commit + GC.  None when the store already holds this step or newer."""
        path = self.upload(step, files, world_size, reuse)
        if path is None or not self.commit(step, path):
            return None
        try:
            self.gc()
        except StoreError:
            pass  # GC is best effort; the next save retries it
        return path

    # ---- read ------------------------------------------------------------
    def manifest(self, path: str) -> dict[str, Any]:
        got = self.b.get(self.k(path, MANIFEST))
        if got is None:
            raise StoreError(f"snapshot {path} has no manifest")
        return json.loads(got[0])

    def fetch(self, path: str, name: str, meta: dict[str, Any], dst: Path) -> None:
        """One file of snapshot `path` into dst (verified, atomic)."""
        check_rel(name)
        src = meta.get("src") or path
        tmp = dst.with_name(f".{dst.name}.cua-part")
        dst.parent.mkdir(parents=True, exist_ok=True)
        if not self.b.get_to(self.k(src, FILES, name), tmp):
            raise StoreError(f"file missing in the store: {src}/{FILES}/{name}")
        if sha256_file(tmp) != meta["sha256"]:
            tmp.unlink(missing_ok=True)
            raise StoreError(f"file corrupt in the store (sha256 mismatch): {src}/{FILES}/{name}")
        os.replace(tmp, dst)

    def download(self, doc: dict[str, Any], dst: Path) -> Path:
        """Fetch LATEST's snapshot into a new dir dst (atomic: complete or absent)."""
        path = doc["path"]
        manifest = self.manifest(path)
        tmp = dst.with_name(dst.name + ".tmp")
        shutil.rmtree(tmp, ignore_errors=True)
        tmp.mkdir(parents=True)
        for name, meta in manifest["files"].items():
            self.fetch(path, name, meta, tmp / name)
        if dst.exists():
            shutil.rmtree(tmp, ignore_errors=True)
            return dst
        os.replace(tmp, dst)
        return dst

    # ---- GC --------------------------------------------------------------
    def gc(self) -> list[str]:
        """Keep the newest `keep` committed snapshots and every object they
        point at; drop older attempts' uncommitted uploads."""
        latest = self.latest()
        if latest is None:
            return []
        keys = self.b.list(self.run_id + "/")
        committed: dict[str, int] = {}
        for k in keys:
            rel = k[len(self.run_id) + 1:]
            if rel.startswith("COMMITTED/step-"):
                got = self.b.get(k)
                if got:
                    try:
                        m = json.loads(got[0])
                        committed[m["path"]] = int(m["step"])
                    except (ValueError, KeyError):
                        pass
        if latest.get("path"):
            committed[latest["path"]] = latest["step"]
        keep_paths = {p for p, _s in sorted(committed.items(), key=lambda x: -x[1])[:self.keep]}
        if latest.get("path"):
            keep_paths.add(latest["path"])
        referenced: set[str] = set()
        for p in keep_paths:
            referenced.add(self.k(p, MANIFEST))
            try:
                files = self.manifest(p)["files"]
            except (StoreError, ValueError, KeyError):
                continue
            for name, meta in files.items():
                referenced.add(self.k(meta.get("src") or p, FILES, name))
        deleted = []
        for k in keys:
            if k in referenced:
                continue
            rel = k[len(self.run_id) + 1:]
            parts = rel.split("/")
            if len(parts) < 3 or not parts[0].startswith("attempt-") or not parts[1].startswith("step-"):
                continue
            sd = f"{parts[0]}/{parts[1]}"
            try:
                attempt = int(parts[0].split("-", 1)[1])
                step_n = int(parts[1].split("-", 1)[1])
            except ValueError:
                continue
            stale_upload = sd not in committed and step_n <= latest["step"]
            if sd in committed or attempt < latest["attempt"] or stale_upload:
                self.b.delete(k)
                deleted.append(k)
        return deleted


def check_rel(name: str) -> str:
    """A relative path inside data_dir (no '', '.', '..' parts, no leading '/')."""
    parts = name.split("/")
    if not name or any(p in ("", ".", "..") for p in parts):
        raise ValueError(f"bad path in data_dir: {name!r}")
    return name


# --------------------------------------------------------------------------
# data_dir <-> store
# --------------------------------------------------------------------------
INTERNAL = ".cua"          # data_dir/.cua/: the runtime's own files, never synced
HOST_FILES = (".cua-last-used",)  # the host's own markers at data_dir's top, never synced
_SKIP_SUFFIXES = (".tmp", ".part", ".cua-part", ".lock", ".partial")


def scan(data_dir: Path) -> dict[str, tuple[int, int]]:
    """Regular files under data_dir: relative path -> (size, mtime_ns).
    Skips data_dir/.cua/, the host's markers (.cua-last-used), symlinks and
    files that look half-written (*.tmp...)."""
    out: dict[str, tuple[int, int]] = {}
    root = str(data_dir)
    for dirpath, dirs, files in os.walk(root):
        if dirpath == root:
            dirs[:] = [d for d in dirs if d != INTERNAL]
        for name in files:
            if name.endswith(_SKIP_SUFFIXES) or (dirpath == root and name in HOST_FILES):
                continue
            full = os.path.join(dirpath, name)
            try:
                st = os.lstat(full)
            except FileNotFoundError:
                continue
            if not os.path.isfile(full) or os.path.islink(full):
                continue
            out[os.path.relpath(full, root).replace(os.sep, "/")] = (st.st_size, st.st_mtime_ns)
    return out


class DataDirSync:
    """Mirror a run's data_dir through the store.

    restore(): every rank, before user code: data_dir becomes exactly the
        newest committed snapshot (rank 0 first claims the run for this
        attempt; the other ranks wait for that claim so all read the same one).
    sync(): rank 0, in the background and at exit: commit the files that
        changed as the next snapshot.  Files written in the last `settle`
        seconds, or that change while being copied, wait for the next sync;
        a file that keeps changing for `max_wait` seconds (a checkpoint
        rewritten every few steps) is taken as soon as one copy of it is
        consistent (its size and mtime did not move while it was copied)."""

    def __init__(self, store: CheckpointStore, data_dir: Path, rank: int = 0, world_size: int = 1,
                 settle: float = 2.0, log=None, max_wait: float = 60.0):
        self.store = store
        self.data_dir = Path(data_dir)
        self.rank = rank
        self.world_size = world_size
        self.settle = settle
        self.log = log or (lambda msg: None)
        # rel -> {"size", "mtime_ns", "sha256", "src"} for files the store holds as they are here
        self.known: dict[str, dict[str, Any]] = {}
        self.committed: set[str] = set()  # the rel paths of the newest snapshot
        self.fenced = False
        self.max_wait = max_wait
        self._unsettled_since: dict[str, int] = {}  # rel -> when a sync first saw it unsettled (ns)
        self._lock = threading.Lock()

    @property
    def internal(self) -> Path:
        return self.data_dir / INTERNAL

    def restore(self, wait_secs: float = 300.0) -> dict[str, Any] | None:
        self.data_dir.mkdir(parents=True, exist_ok=True)
        if self.rank == 0:
            doc = self.store.claim()
        else:
            deadline = time.monotonic() + wait_secs
            while True:
                doc = self.store.latest()
                if doc and doc["attempt"] >= self.store.attempt:
                    break
                if time.monotonic() > deadline:
                    raise StoreError(f"rank 0 did not claim the run in the store within {wait_secs:.0f}s")
                time.sleep(0.5)
        files: dict[str, Any] = {}
        if doc and doc.get("path"):
            files = self.store.manifest(doc["path"])["files"]
        here = scan(self.data_dir)
        removed = 0
        for name in here:
            if name not in files:  # not in the newest snapshot: another attempt's leftovers
                (self.data_dir / name).unlink(missing_ok=True)
                removed += 1
        _prune_empty_dirs(self.data_dir)
        fetched = 0
        for name, meta in sorted(files.items()):
            dst = self.data_dir / name
            if name in here and here[name][0] == meta["size"] and sha256_file(dst) == meta["sha256"]:
                pass
            else:
                if dst.is_dir():
                    shutil.rmtree(dst)
                self.store.fetch(doc["path"], name, meta, dst)
                fetched += 1
            st = dst.stat()
            self.known[name] = {"size": st.st_size, "mtime_ns": st.st_mtime_ns, "sha256": meta["sha256"],
                                "src": meta.get("src") or doc["path"]}
        self.committed = set(files)
        if doc and doc.get("path"):
            self.log(f"data_dir restored from snapshot {doc['step']} ({len(files)} files, "
                     f"{fetched} downloaded, {removed} stale removed)")
        elif removed:
            self.log(f"data_dir: no snapshot yet; removed {removed} files of an earlier attempt")
        return doc

    def sync(self, final: bool = False) -> str | None:
        """Commit what changed as the next snapshot (rank 0).  None: nothing new.
        `final`: the user code ended, so files just written count too."""
        with self._lock:
            if self.fenced:
                return None
            try:
                return self._sync(0.0 if final else self.settle)
            except StaleAttempt as e:
                self.fenced = True
                self.log(f"data_dir sync stopped: {e}")
                return None

    def _sync(self, settle: float) -> str | None:
        now_ns = time.time_ns()
        here = scan(self.data_dir)
        new: dict[str, Path] = {}
        reuse: dict[str, dict[str, Any]] = {}
        stage = self.internal / "stage"
        shutil.rmtree(stage, ignore_errors=True)
        staged_meta: dict[str, tuple[int, int]] = {}
        for name, (size, mtime) in sorted(here.items()):
            k = self.known.get(name)
            if k and k["size"] == size and k["mtime_ns"] == mtime:
                reuse[name] = k
                continue
            if now_ns - mtime < settle * 1e9:
                since = self._unsettled_since.setdefault(name, now_ns)
                if now_ns - since < self.max_wait * 1e9:
                    if k:  # still being written: keep the previous version for now
                        reuse[name] = k
                    continue
            src = self.data_dir / name
            dst = stage / name
            dst.parent.mkdir(parents=True, exist_ok=True)
            try:
                shutil.copyfile(src, dst)
                st = src.stat()
            except FileNotFoundError:
                continue
            if (st.st_size, st.st_mtime_ns) != (size, mtime):  # changed while copying
                dst.unlink(missing_ok=True)
                if k:
                    reuse[name] = k
                continue
            new[name] = dst
            staged_meta[name] = (size, mtime)
            self._unsettled_since.pop(name, None)
        if not new and set(reuse) == self.committed:
            shutil.rmtree(stage, ignore_errors=True)
            return None
        latest = self.store.latest()
        if latest and latest["attempt"] > self.store.attempt:
            raise StaleAttempt(f"attempt {self.store.attempt} is stale: attempt {latest['attempt']} owns the run")
        step = (latest["step"] if latest else 0) + 1
        path = self.store.save(step, new, self.world_size, reuse)
        if path is None:
            raise StaleAttempt(f"snapshot {step} of attempt {self.store.attempt} was not committed")
        manifest_files = self.store.manifest(path)["files"]
        for name, (size, mtime) in staged_meta.items():
            meta = manifest_files[name]
            self.known[name] = {"size": size, "mtime_ns": mtime, "sha256": meta["sha256"], "src": meta["src"]}
        for name in list(self.known):
            if name not in manifest_files:
                del self.known[name]
        self.committed = set(manifest_files)
        shutil.rmtree(stage, ignore_errors=True)
        return path


def _prune_empty_dirs(root: Path) -> None:
    for dirpath, dirs, files in os.walk(root, topdown=False):
        p = Path(dirpath)
        if p == root or INTERNAL in p.relative_to(root).parts[:1]:
            continue
        if not dirs and not files:
            try:
                p.rmdir()
            except OSError:
                pass
