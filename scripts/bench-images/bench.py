#!/usr/bin/env python3
"""Benchmark image pipeline helper (stdlib only).

    bench.py list
    bench.py validate [ID ...]
    bench.py get ID FIELD.PATH            # print one field (scripts)
    bench.py fetch ID [--dest DIR]        # download sources, verify sha256, unpack
    bench.py split-tar IN.tar OUTDIR [--group NAME=top1,top2 ...] [--exclude PATH ...]
    bench.py disk-layer DISK.img OUT.tar   # containerDisk layer (/disk/disk.img, uid 107)
    bench.py layout OUTDIR --platform linux/amd64 --config CONFIG.json --layer META.json ...
    bench.py docker-layout SAVE.tar OUTDIR --platform linux/arm64 [--label K=V ...]
    bench.py labels ID --variant rootfs|containerdisk [--stamp S]
    bench.py index ID --stamp S --rootfs DESC.json ... --disk DESC.json ... [--disk-digest D] OUTDIR
    bench.py lock ID --stamp S --index DIGEST --disk-index DIGEST [--child ...]

A benchmark is described once in ``libs/images/bench/<id>/bench.json``
(sources pinned by sha256 / Hugging Face revision, license, redistribution
class, server, variants). Everything here is a pure function of that file
plus the build outputs, so CI and a laptop produce the same documents.

Layer metadata files (``--layer``) are JSON ``{"path", "mediaType", "digest",
"size", "diff_id"}``; ``split-tar`` and ``disk-layer`` write them next to
each layer.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import time
import urllib.request
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
BENCH_ROOT = REPO / "libs" / "images" / "bench"

OCI_INDEX = "application/vnd.oci.image.index.v1+json"
OCI_MANIFEST = "application/vnd.oci.image.manifest.v1+json"
OCI_CONFIG = "application/vnd.oci.image.config.v1+json"
LAYER_TAR = "application/vnd.oci.image.layer.v1.tar"
LAYER_GZIP = "application/vnd.oci.image.layer.v1.tar+gzip"
DOCKER_MANIFEST = "application/vnd.docker.distribution.manifest.v2+json"
DOCKER_INDEX = "application/vnd.docker.distribution.manifest.list.v2+json"

VARIANTS_ANNOTATION = "ai.cua.image.variants"
CONTAINERDISK_UID = 107
SOURCE_LABEL = "org.opencontainers.image.source"

REDISTRIBUTION = {
    # class -> (may we push it, visibility)
    "public": (True, "public"),
    "private-only": (True, "private"),
    "reference-upstream": (False, None),
    "user-build-only": (False, None),
}
SOURCE_KINDS = {"qcow2", "dockerfile", "dataset", "none", "deb", "wheel"}
VARIANTS = ("rootfs", "containerdisk", "lume")
REQUIREMENT_WORDS = {"kvm", "egress", "privileged", "openai", "hf-gated"}
TAG_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
IMMUTABLE_RE = re.compile(r"-[0-9]{8}-[0-9a-f]{7}(-(amd64|arm64))?$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


class BenchError(Exception):
    pass


# ── bench.json ────────────────────────────────────────────────────────────


def bench_ids() -> list[str]:
    return sorted(p.parent.name for p in BENCH_ROOT.glob("*/bench.json"))


def load(bench_id: str) -> dict:
    path = BENCH_ROOT / bench_id / "bench.json"
    if not path.is_file():
        raise BenchError(f"no {path.relative_to(REPO)} (known: {', '.join(bench_ids())})")
    data = json.loads(path.read_text())
    problems = validate(data, bench_id)
    if problems:
        raise BenchError(f"{path.relative_to(REPO)}: " + "; ".join(problems))
    return data


def validate(b: dict, bench_id: str | None = None) -> list[str]:
    """Problems with a bench.json document (empty when valid)."""
    p: list[str] = []

    def need(key: str, typ: type) -> object:
        v = b.get(key)
        if not isinstance(v, typ):
            p.append(f"{key}: expected {typ.__name__}")
        return v

    if b.get("schema_version") != 1:
        p.append("schema_version must be 1")
    bid = need("id", str)
    if bench_id and bid != bench_id:
        p.append(f"id {bid!r} does not match its directory {bench_id!r}")
    ver = need("version", str)
    if isinstance(ver, str):
        if not TAG_RE.match(ver) or ver.endswith("-disk") or IMMUTABLE_RE.search(ver):
            p.append(f"version {ver!r} must be a plain tag word (not -disk, not a pin)")
        if ver in {"latest", "main", "stable", "edge", "nightly"}:
            p.append(f"version {ver!r} is reserved")
    need("name", str)
    kind = b.get("kind", "image")
    if kind not in {"image", "dataset"}:
        p.append("kind must be image or dataset")
    lic = need("license", dict)
    if isinstance(lic, dict) and not isinstance(lic.get("code"), str):
        p.append("license.code: expected str (SPDX id or 'none')")
    red = b.get("redistribution")
    if red not in REDISTRIBUTION:
        p.append(f"redistribution must be one of {sorted(REDISTRIBUTION)}")
    upstream = need("upstream", dict)
    if isinstance(upstream, dict):
        if not str(upstream.get("repo", "")).startswith("https://"):
            p.append("upstream.repo: expected an https URL")
        commit = str(upstream.get("commit", ""))
        if commit and not re.fullmatch(r"[0-9a-f]{40}", commit):
            p.append("upstream.commit: expected a full 40-hex commit")
    for i, src in enumerate(b.get("sources") or []):
        where = f"sources[{i}]"
        if src.get("kind") not in SOURCE_KINDS:
            p.append(f"{where}.kind must be one of {sorted(SOURCE_KINDS)}")
        if "hf" in src:
            hf = src["hf"]
            if not re.fullmatch(r"[0-9a-f]{40}", str(hf.get("revision", ""))):
                p.append(f"{where}.hf.revision: expected a full commit sha (not a branch)")
            for k in ("repo", "file"):
                if not hf.get(k):
                    p.append(f"{where}.hf.{k} is required")
        elif "url" in src:
            if not str(src["url"]).startswith("https://"):
                p.append(f"{where}.url: expected https")
        elif "git" in src:
            if not re.fullmatch(r"[0-9a-f]{40}", str(src["git"].get("commit", ""))):
                p.append(f"{where}.git.commit: expected a full commit sha")
        else:
            p.append(f"{where}: needs hf, url or git")
        if "git" not in src and not SHA256_RE.match(str(src.get("sha256", ""))):
            p.append(f"{where}.sha256: expected 64 hex chars")
    if kind == "dataset":
        return p
    repo = need("repository", str)
    if isinstance(repo, str) and not re.fullmatch(r"ghcr\.io/trycua/bench-[a-z0-9-]+", repo):
        p.append("repository must be ghcr.io/trycua/bench-<id> (new repos only; TAG SAFETY)")
    os_ = b.get("os")
    if os_ not in {"linux", "windows", "macos"}:
        p.append("os must be linux, windows or macos")
    arch = b.get("arch")
    if not isinstance(arch, list) or not arch or any(a not in {"amd64", "arm64"} for a in arch):
        p.append("arch: a non-empty list of amd64/arm64")
    variants = b.get("variants")
    if not isinstance(variants, list) or not variants or any(v not in VARIANTS for v in variants):
        p.append(f"variants: a non-empty list of {VARIANTS}")
    else:
        if os_ == "linux" and set(variants) != {"rootfs", "containerdisk"}:
            p.append("linux benchmarks ship both rootfs and containerdisk")
        if os_ in {"windows", "macos"} and "rootfs" in variants:
            p.append(f"{os_} benchmarks are VM-only (no rootfs)")
    req = b.get("requires", {})
    if not isinstance(req, dict):
        p.append("requires: a map variant -> [words]")
    else:
        for v, words in req.items():
            if v not in (variants or []):
                p.append(f"requires.{v}: not a declared variant")
            bad = [w for w in words if w not in REQUIREMENT_WORDS and not str(w).startswith("env:")]
            if bad:
                p.append(f"requires.{v}: unknown words {bad}")
    server = b.get("server")
    if server is not None:
        if not isinstance(server, dict) or not isinstance(server.get("port"), int):
            p.append("server: {name, port, protocol, health}")
        elif server.get("protocol", "http") not in {"http", "https", "tcp"}:
            p.append("server.protocol must be http, https or tcp")
    if not isinstance(b.get("spacesd"), bool):
        p.append("spacesd: expected true/false (what the image runs)")
    build = b.get("build")
    if not isinstance(build, dict) or build.get("kind") not in {"qcow2", "dockerfile"}:
        p.append("build.kind must be qcow2 or dockerfile")
    push_ok, visibility = REDISTRIBUTION.get(red, (False, None))
    if b.get("visibility") not in {"public", "private"}:
        p.append("visibility must be public or private")
    elif push_ok and b.get("visibility") != visibility:
        p.append(f"redistribution {red} publishes {visibility} packages, not {b.get('visibility')}")
    return p


def field(b: dict, path: str) -> object:
    cur: object = b
    for part in path.split("."):
        if isinstance(cur, list):
            cur = cur[int(part)]
        elif isinstance(cur, dict):
            if part not in cur:
                raise BenchError(f"no field {path}")
            cur = cur[part]
        else:
            raise BenchError(f"no field {path}")
    return cur


# ── fetch ─────────────────────────────────────────────────────────────────


def sha256_file(path: Path, chunk: int = 8 << 20) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            b = f.read(chunk)
            if not b:
                return h.hexdigest()
            h.update(b)


def _download(url: str, dest: Path, headers: dict[str, str]) -> None:
    tmp = dest.with_suffix(dest.suffix + ".part")
    req = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(req, timeout=120) as r, open(tmp, "wb") as f:
        shutil.copyfileobj(r, f, 8 << 20)
    tmp.replace(dest)


def fetch_source(src: dict, dest: Path) -> Path:
    """Download one source into ``dest`` and verify it; returns the file path."""
    dest.mkdir(parents=True, exist_ok=True)
    if "git" in src:
        g = src["git"]
        out = dest / src.get("name", Path(g["repo"]).stem)
        if not (out / ".git").is_dir():
            subprocess.run(["git", "init", "-q", str(out)], check=True)
            subprocess.run(["git", "-C", str(out), "remote", "add", "origin", g["repo"]], check=True)
        subprocess.run(
            ["git", "-C", str(out), "fetch", "-q", "--depth", "1", "origin", g["commit"]], check=True
        )
        subprocess.run(["git", "-C", str(out), "checkout", "-q", "--detach", g["commit"]], check=True)
        head = subprocess.run(
            ["git", "-C", str(out), "rev-parse", "HEAD"], check=True, capture_output=True, text=True
        ).stdout.strip()
        if head != g["commit"]:
            raise BenchError(f"{g['repo']}: HEAD {head} != pinned {g['commit']}")
        return out
    if "hf" in src:
        hf = src["hf"]
        name = Path(hf["file"]).name
        target = dest / name
        url = (
            f"https://huggingface.co/{'datasets/' if hf.get('repo_type', 'dataset') == 'dataset' else ''}"
            f"{hf['repo']}/resolve/{hf['revision']}/{hf['file']}"
        )
    else:
        target = dest / (src.get("name") or Path(src["url"]).name)
        url = src["url"]
    marker = target.with_suffix(target.suffix + ".sha256-ok")
    want = src["sha256"]
    if target.is_file() and marker.is_file() and marker.read_text().strip() == want:
        print(f"cached  {target}")
    else:
        if not target.is_file() or sha256_file(target) != want:
            headers = {"User-Agent": "cua-bench-images/1"}
            # Gated Hugging Face sources: HF_TOKEN from the environment, never echoed.
            if "hf" in src and os.environ.get("HF_TOKEN"):
                headers["Authorization"] = "Bearer " + os.environ["HF_TOKEN"]
            if "hf" in src and shutil.which("hf") and not os.environ.get("HF_TOKEN"):
                # The hf CLI uses the cached login (gated sets) and resumes.
                hf = src["hf"]
                subprocess.run(
                    ["hf", "download", hf["repo"], hf["file"], "--repo-type",
                     hf.get("repo_type", "dataset"), "--revision", hf["revision"],
                     "--local-dir", str(dest)],
                    check=True,
                )
                got = dest / hf["file"]
                if got != target:
                    got.replace(target)
            else:
                print(f"fetch   {url}")
                _download(url, target, headers)
        got = sha256_file(target)
        if got != want:
            raise BenchError(f"{target}: sha256 {got} != pinned {want}")
        marker.write_text(want + "\n")
        print(f"ok      {target} sha256:{want}")
    unpack = src.get("unpack")
    if unpack == "zip":
        inner = dest / src["inner"]
        if not inner.is_file() or inner.stat().st_mtime < target.stat().st_mtime:
            print(f"unzip   {target.name} -> {inner.name}")
            with zipfile.ZipFile(target) as z:
                info = z.getinfo(src["inner"])
                with z.open(info) as r, open(str(inner) + ".part", "wb") as w:
                    shutil.copyfileobj(r, w, 8 << 20)
            Path(str(inner) + ".part").replace(inner)
        return inner
    return target


def cmd_fetch(args: argparse.Namespace) -> None:
    b = load(args.id)
    dest = Path(args.dest).expanduser() / b["id"]
    paths = {}
    for src in b.get("sources") or []:
        paths[src.get("role", src.get("name", "source"))] = str(fetch_source(src, dest / "source"))
    (dest / "sources.json").write_text(json.dumps(paths, indent=2) + "\n")
    print(json.dumps(paths, indent=2))


# ── layers ────────────────────────────────────────────────────────────────


class _HashWriter(io.RawIOBase):
    """A file wrapper hashing and counting what passes through."""

    def __init__(self, f):
        self.f, self.h, self.n = f, hashlib.sha256(), 0

    def writable(self) -> bool:
        return True

    def write(self, b) -> int:
        self.h.update(b)
        self.n += len(b)
        self.f.write(b)
        return len(b)


class _Layer:
    def __init__(self, path: Path, compress: bool):
        self.path = path
        self.raw = open(path, "wb")
        self.outer = _HashWriter(self.raw)
        self.gz = gzip.GzipFile(fileobj=self.outer, mode="wb", compresslevel=6, mtime=0) if compress else None
        self.inner = _HashWriter(self.gz if self.gz else self.outer)
        self.tar = tarfile.open(fileobj=self.inner, mode="w|", format=tarfile.PAX_FORMAT)
        self.compress = compress

    def close(self) -> dict:
        self.tar.close()
        if self.gz:
            self.gz.close()
        self.raw.close()
        meta = {
            "path": str(self.path),
            "mediaType": LAYER_GZIP if self.compress else LAYER_TAR,
            "digest": "sha256:" + self.outer.h.hexdigest(),
            "size": self.outer.n,
            "diff_id": "sha256:" + self.inner.h.hexdigest(),
        }
        Path(str(self.path) + ".json").write_text(json.dumps(meta, indent=2) + "\n")
        return meta


def _norm(name: str) -> str:
    n = name
    while n.startswith("./"):
        n = n[2:]
    return n.lstrip("/")


def split_tar(src: Path, out: Path, groups: list[tuple[str, list[str]]], excludes: list[str],
              compress: bool = True) -> list[dict]:
    """Stream ``src`` into several layers by top-level directory.

    Each group is ``name -> [top-level names]``; everything else goes to the
    last group, ``rest``. Excluded paths (and everything below them) are
    dropped; their directory entries are kept empty so mount points exist.
    """
    out.mkdir(parents=True, exist_ok=True)
    names = [g for g, _ in groups] + ["rest"]
    layers = {g: _Layer(out / f"{i:02d}-{g}.tar{'.gz' if compress else ''}", compress)
              for i, g in enumerate(names)}
    top_to_group = {t: g for g, tops in groups for t in tops}
    ex = [_norm(e).rstrip("/") for e in excludes]
    dropped = 0
    with tarfile.open(src, mode="r|*") as t:
        for m in t:
            n = _norm(m.name)
            if not n:
                continue
            hit = next((e for e in ex if n == e or n.startswith(e + "/")), None)
            if hit is not None and not (n == hit and m.isdir()):
                dropped += 1
                continue
            m.name = n
            if m.islnk():
                m.linkname = _norm(m.linkname)
            top = n.split("/", 1)[0]
            layer = layers[top_to_group.get(top, "rest")]
            if m.isfile():
                layer.tar.addfile(m, t.extractfile(m))
            else:
                layer.tar.addfile(m)
    metas = [layers[g].close() for g in names]
    print(f"split {src} -> {len(metas)} layers, dropped {dropped} excluded entries", file=sys.stderr)
    return metas


def disk_layer(disk: Path, out: Path) -> dict:
    """An uncompressed containerDisk layer: disk/ and disk/disk.img, uid/gid 107.

    The qcow2 is already compressed, so the layer is plain tar (no second
    compression pass over tens of GB)."""
    layer = _Layer(out, compress=False)
    d = tarfile.TarInfo("disk")
    d.type, d.mode, d.uid, d.gid, d.mtime = tarfile.DIRTYPE, 0o555, CONTAINERDISK_UID, CONTAINERDISK_UID, 0
    layer.tar.addfile(d)
    f = tarfile.TarInfo("disk/disk.img")
    f.size, f.mode, f.uid, f.gid, f.mtime = disk.stat().st_size, 0o440, CONTAINERDISK_UID, CONTAINERDISK_UID, 0
    with open(disk, "rb") as r:
        layer.tar.addfile(f, r)
    return layer.close()


def deb_members(deb: Path) -> dict[str, bytes]:
    """Regular files of a .deb's data archive: {path without ./: bytes}.

    A .deb is an ar archive; its data.tar is gzip, xz or zstd compressed
    (Ubuntu uses zstd). zstd needs Python 3.14's ``compression.zstd``, the
    ``zstandard`` module, or the ``zstd`` command."""
    raw = deb.read_bytes()
    if raw[:8] != b"!<arch>\n":
        raise BenchError(f"{deb}: not an ar archive")
    pos, data, name = 8, None, None
    while pos + 60 <= len(raw):
        header = raw[pos:pos + 60]
        name = header[:16].decode().strip().rstrip("/")
        size = int(header[48:58].decode().strip())
        body = raw[pos + 60:pos + 60 + size]
        pos += 60 + size + (size % 2)
        if name.startswith("data.tar"):
            data = body
            break
    if data is None:
        raise BenchError(f"{deb}: no data.tar member")
    if name.endswith(".zst"):
        try:
            from compression import zstd  # Python 3.14+
            data = zstd.decompress(data)
        except ImportError:
            try:
                import zstandard
                data = zstandard.ZstdDecompressor().decompressobj().decompress(data)
            except ImportError:
                data = subprocess.run(["zstd", "-dc"], input=data, capture_output=True, check=True).stdout
    elif name.endswith(".xz"):
        import lzma
        data = lzma.decompress(data)
    elif name.endswith(".gz"):
        data = gzip.decompress(data)
    out = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as t:
        for m in t:
            if m.isfile():
                out[_norm(m.name)] = t.extractfile(m).read()
    return out


# ── OCI documents ─────────────────────────────────────────────────────────


def canonical_json(obj: object) -> bytes:
    return json.dumps(obj, separators=(",", ":"), sort_keys=False).encode()


def write_blob(layout: Path, data: bytes) -> str:
    dig = hashlib.sha256(data).hexdigest()
    p = layout / "blobs" / "sha256" / dig
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(data)
    return "sha256:" + dig


def _link_blob(layout: Path, meta: dict) -> None:
    dst = layout / "blobs" / "sha256" / meta["digest"].split(":", 1)[1]
    dst.parent.mkdir(parents=True, exist_ok=True)
    if dst.exists():
        return
    try:
        os.link(meta["path"], dst)
    except OSError:
        shutil.copyfile(meta["path"], dst)


def platform_of(s: str) -> dict:
    parts = s.split("/")
    if len(parts) < 2 or parts[0] != "linux":
        raise BenchError(f"platform {s!r}: expected linux/<arch>[/<variant>]")
    p = {"architecture": parts[1], "os": parts[0]}
    if len(parts) > 2:
        p["variant"] = parts[2]
    return p


def write_layout(out: Path, platform: str, config: dict, layers: list[dict],
                 annotations: dict | None = None) -> dict:
    """An OCI layout with one image manifest; returns its descriptor."""
    out.mkdir(parents=True, exist_ok=True)
    (out / "oci-layout").write_text('{"imageLayoutVersion":"1.0.0"}')
    plat = platform_of(platform)
    config = dict(config)
    config.setdefault("architecture", plat["architecture"])
    config.setdefault("os", "linux")
    config["rootfs"] = {"type": "layers", "diff_ids": [m["diff_id"] for m in layers]}
    cfg = canonical_json(config)
    cdig = write_blob(out, cfg)
    for m in layers:
        _link_blob(out, m)
    manifest = {
        "schemaVersion": 2,
        "mediaType": OCI_MANIFEST,
        "config": {"mediaType": OCI_CONFIG, "digest": cdig, "size": len(cfg)},
        "layers": [{"mediaType": m["mediaType"], "digest": m["digest"], "size": m["size"]} for m in layers],
    }
    if annotations:
        manifest["annotations"] = annotations
    mb = canonical_json(manifest)
    mdig = write_blob(out, mb)
    desc = {"mediaType": OCI_MANIFEST, "digest": mdig, "size": len(mb),
            "platform": {k: v for k, v in plat.items() if k != "variant"}}
    (out / "index.json").write_text(json.dumps(
        {"schemaVersion": 2, "mediaType": OCI_INDEX, "manifests": [desc]}, indent=2) + "\n")
    return desc


def docker_layout(save_tar: Path, out: Path, platform: str, labels: dict[str, str],
                  drop: tuple[str, ...] = ()) -> dict:
    """Re-pack one platform of a `docker save` (OCI) archive as a single OCI
    manifest whose config carries ``labels`` (layers unchanged)."""
    work = out.with_name(out.name + ".save")
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    with tarfile.open(save_tar) as t:
        t.extractall(work, filter="data") if hasattr(tarfile, "data_filter") else t.extractall(work)
    plat = platform_of(platform)

    def blob(d: str) -> Path:
        return work / "blobs" / "sha256" / d.split(":", 1)[1]

    def find_manifest(desc: dict) -> dict:
        mt = desc.get("mediaType")
        doc = json.loads(blob(desc["digest"]).read_text())
        if mt in (OCI_INDEX, DOCKER_INDEX) or "manifests" in doc:
            for d in doc["manifests"]:
                p = d.get("platform") or {}
                if (d.get("annotations") or {}).get("vnd.docker.reference.type") == "attestation-manifest":
                    continue
                if p.get("architecture") == plat["architecture"] and p.get("os") == "linux":
                    return find_manifest(d)
            raise BenchError(f"{save_tar}: no {platform} manifest")
        return doc

    index = json.loads((work / "index.json").read_text())
    manifest = find_manifest(index["manifests"][0])
    config = json.loads(blob(manifest["config"]["digest"]).read_text())
    cfg = config.setdefault("config", {})
    cfg["Labels"] = {k: v for k, v in {**(cfg.get("Labels") or {}), **labels}.items() if k not in drop}
    layers = []
    for l in manifest["layers"]:
        layers.append({"path": str(blob(l["digest"])), "mediaType": _oci_layer_type(l["mediaType"]),
                       "digest": l["digest"], "size": l["size"]})
    diff_ids = config["rootfs"]["diff_ids"]
    for m, d in zip(layers, diff_ids):
        m["diff_id"] = d
    desc = write_layout(out, platform, config, layers)
    shutil.rmtree(work)
    return desc


def _oci_layer_type(mt: str) -> str:
    return {
        "application/vnd.docker.image.rootfs.diff.tar.gzip": LAYER_GZIP,
        "application/vnd.docker.image.rootfs.diff.tar": LAYER_TAR,
    }.get(mt, mt)


# ── labels, annotations, indexes ──────────────────────────────────────────


def labels(b: dict, variant: str, stamp: str | None = None) -> dict[str, str]:
    """Config labels / manifest annotations for one variant of a benchmark."""
    spacesd = "true" if b["spacesd"] else "false"
    out = {
        "ai.cua.image.os": b["os"],
        "ai.cua.spacesd": spacesd,
        "ai.cua.env-driver": spacesd,
        "ai.cua.image.variant": variant,
        "ai.cua.image.requires": json.dumps(sorted(b.get("requires", {}).get(variant, [])),
                                            separators=(",", ":")),
        "ai.cua.bench.id": b["id"],
        "ai.cua.bench.version": b["version"],
        "org.opencontainers.image.title": f"bench-{b['id']}",
        "org.opencontainers.image.description": b["name"],
        "org.opencontainers.image.licenses": b["license"]["code"],
        "org.opencontainers.image.version": b["version"],
    }
    # ghcr links a package whose manifests name a source repository to that
    # repository (it then inherits the repository's access settings; its
    # visibility stays as created, private, until an org admin changes it).
    # Public benchmarks are linked; private ones never carry the label, so
    # nothing about the public repository's access reaches their packages.
    if b.get("visibility") == "public":
        out[SOURCE_LABEL] = "https://github.com/trycua/cua"
    if b.get("server"):
        out["ai.cua.bench.server"] = json.dumps(b["server"], separators=(",", ":"))
    if b.get("sources"):
        out["ai.cua.image.source"] = json.dumps(
            [_source_pin(s) for s in b["sources"] if s.get("role", "image") == "image"] or
            [_source_pin(s) for s in b["sources"]], separators=(",", ":"))
    if stamp:
        out["org.opencontainers.image.revision"] = stamp.rsplit("-", 1)[-1]
    return out


def _source_pin(s: dict) -> str:
    if "hf" in s:
        h = s["hf"]
        return f"hf://{h.get('repo_type', 'dataset')}s/{h['repo']}@{h['revision']}/{h['file']}#sha256:{s['sha256']}"
    if "git" in s:
        return f"{s['git']['repo']}@{s['git']['commit']}"
    return f"{s['url']}#sha256:{s['sha256']}"


def build_indexes(b: dict, stamp: str, rootfs: list[dict], disks: list[dict],
                  disk_digest: str | None = None) -> tuple[dict, dict | None]:
    """The two cross-linked indexes, as ghcr.io/trycua/linux publishes them:

    * ``<ver>-disk-<stamp>``: the containerDisk children (KubeVirt / QEMU
      pull this), ``ai.cua.image.variants = {"rootfs": "<repo>:<ver>-<stamp>"}``;
    * ``<ver>-<stamp>``: the rootfs children (docker / gVisor),
      ``ai.cua.image.variants = {"containerdisk": "<repo>@<disk index digest>"}``.

    One index per variant because docker and containerd select the first
    child matching the platform and ignore annotations, and ctr pulls every
    matching child's layers, so a single multi-variant index over-pulls.
    The disk index is pushed first; the rootfs index names it by digest, so
    it is only built once ``disk_digest`` is known (None until then).
    ``rootfs``/``disks`` are manifest descriptors (digest, size, mediaType,
    platform). VM-only benchmarks have no rootfs index."""
    repo, ver = b["repository"], b["version"]
    common = {k: v for k, v in labels(b, "rootfs", stamp).items()
              if k not in ("ai.cua.image.variant", "ai.cua.image.requires")}
    req = b.get("requires", {})

    def child(d: dict, variant: str) -> dict:
        return {"mediaType": d["mediaType"], "digest": d["digest"], "size": d["size"],
                "platform": {"architecture": d["platform"]["architecture"], "os": "linux"},
                "annotations": {"ai.cua.image.variant": variant}}

    def annotations(variant: str, links: dict) -> dict:
        out = {**common, "ai.cua.image.variant": variant,
               "ai.cua.image.requires": json.dumps(sorted(req.get(variant, [])), separators=(",", ":"))}
        if links:
            out[VARIANTS_ANNOTATION] = json.dumps(links, separators=(",", ":"))
        return out

    disk_links = {"rootfs": f"{repo}:{ver}-{stamp}"} if rootfs else {}
    disk_index = {"schemaVersion": 2, "mediaType": OCI_INDEX,
                  "manifests": [child(d, "containerdisk") for d in disks],
                  "annotations": annotations("containerdisk", disk_links)}
    rootfs_index = None
    if rootfs and disk_digest:
        rootfs_index = {"schemaVersion": 2, "mediaType": OCI_INDEX,
                        "manifests": [child(d, "rootfs") for d in rootfs],
                        "annotations": annotations("rootfs", {"containerdisk": f"{repo}@{disk_digest}"})}
    return disk_index, rootfs_index


# ── CLI ───────────────────────────────────────────────────────────────────


def _desc(path: str) -> dict:
    d = json.loads(Path(path).read_text())
    for k in ("digest", "size", "mediaType", "platform"):
        if k not in d:
            raise BenchError(f"{path}: descriptor needs {k}")
    return d


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="bench.py", description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list")
    v = sub.add_parser("validate")
    v.add_argument("ids", nargs="*")
    g = sub.add_parser("get")
    g.add_argument("id")
    g.add_argument("path")
    f = sub.add_parser("fetch")
    f.add_argument("id")
    f.add_argument("--dest", default=os.environ.get("CUA_BENCH_WORK", "~/.cache/cua-bench-images"))
    s = sub.add_parser("split-tar")
    s.add_argument("src")
    s.add_argument("out")
    s.add_argument("--group", action="append", default=[])
    s.add_argument("--exclude", action="append", default=[])
    s.add_argument("--no-compress", action="store_true")
    d = sub.add_parser("disk-layer")
    d.add_argument("disk")
    d.add_argument("out")
    lo = sub.add_parser("layout")
    lo.add_argument("out")
    lo.add_argument("--platform", required=True)
    lo.add_argument("--config", required=True)
    lo.add_argument("--layer", action="append", default=[])
    lo.add_argument("--label", action="append", default=[])
    dl = sub.add_parser("docker-layout")
    dl.add_argument("save")
    dl.add_argument("out")
    dl.add_argument("--platform", required=True)
    dl.add_argument("--label", action="append", default=[])
    dl.add_argument("--drop-label", action="append", default=[])
    la = sub.add_parser("labels")
    la.add_argument("id")
    la.add_argument("--variant", required=True, choices=VARIANTS)
    la.add_argument("--stamp")
    la.add_argument("--format", choices=["json", "docker-args"], default="json")
    ix = sub.add_parser("index")
    ix.add_argument("id")
    ix.add_argument("--stamp", required=True)
    ix.add_argument("--rootfs", action="append", default=[])
    ix.add_argument("--disk", action="append", default=[])
    ix.add_argument("--disk-digest", help="digest of the pushed disk index (the rootfs index links it)")
    ix.add_argument("out")
    lk = sub.add_parser("lock")
    lk.add_argument("id")
    lk.add_argument("--from", dest="src", required=True, help="publish record JSON")
    args = ap.parse_args(argv)

    try:
        if args.cmd == "list":
            for i in bench_ids():
                b = json.loads((BENCH_ROOT / i / "bench.json").read_text())
                print(f"{i:12s} {b.get('kind', 'image'):8s} {b.get('redistribution', '?'):18s} {b.get('repository', '-')}")
        elif args.cmd == "validate":
            bad = 0
            for i in args.ids or bench_ids():
                path = BENCH_ROOT / i / "bench.json"
                problems = validate(json.loads(path.read_text()), i)
                print(("ok   " if not problems else "FAIL ") + i + ("" if not problems else ": " + "; ".join(problems)))
                bad += bool(problems)
            return 1 if bad else 0
        elif args.cmd == "get":
            val = field(load(args.id), args.path)
            print(val if isinstance(val, str) else json.dumps(val))
        elif args.cmd == "fetch":
            cmd_fetch(args)
        elif args.cmd == "split-tar":
            groups = []
            for spec in args.group:
                name, _, tops = spec.partition("=")
                groups.append((name, [t for t in tops.split(",") if t]))
            metas = split_tar(Path(args.src), Path(args.out), groups, args.exclude, not args.no_compress)
            print(json.dumps(metas, indent=2))
        elif args.cmd == "disk-layer":
            print(json.dumps(disk_layer(Path(args.disk), Path(args.out)), indent=2))
        elif args.cmd == "layout":
            config = json.loads(Path(args.config).read_text())
            lab = dict(kv.split("=", 1) for kv in args.label)
            if lab:
                config.setdefault("config", {}).setdefault("Labels", {}).update(lab)
            metas = []
            for m in args.layer:
                meta = json.loads(Path(m).read_text())
                # Written inside a build container: the layer sits next to its
                # metadata file whatever the recorded path says.
                sibling = Path(m).with_suffix("")
                if not Path(meta["path"]).is_file() and sibling.is_file():
                    meta["path"] = str(sibling)
                metas.append(meta)
            print(json.dumps(write_layout(Path(args.out), args.platform, config, metas), indent=2))
        elif args.cmd == "docker-layout":
            lab = dict(kv.split("=", 1) for kv in args.label)
            print(json.dumps(docker_layout(Path(args.save), Path(args.out), args.platform, lab,
                                           tuple(args.drop_label)), indent=2))
        elif args.cmd == "labels":
            lab = labels(load(args.id), args.variant, args.stamp)
            if args.format == "json":
                print(json.dumps(lab, indent=2))
            else:
                print("\n".join(f"--label\n{k}={v}" for k, v in lab.items()))
        elif args.cmd == "index":
            b = load(args.id)
            if not IMMUTABLE_RE.search("x-" + args.stamp):
                raise BenchError(f"stamp {args.stamp!r}: expected <yyyymmdd>-<sha7>")
            disk_index, rootfs_index = build_indexes(
                b, args.stamp, [_desc(p) for p in args.rootfs], [_desc(p) for p in args.disk],
                args.disk_digest)
            out = Path(args.out)
            out.mkdir(parents=True, exist_ok=True)
            (out / "disk-index.json").write_bytes(canonical_json(disk_index))
            if rootfs_index is not None:
                (out / "index.json").write_bytes(canonical_json(rootfs_index))
            print(out)
        elif args.cmd == "lock":
            b = load(args.id)
            rec = json.loads(Path(args.src).read_text())
            lock = {"id": b["id"], "version": b["version"], "repository": b["repository"],
                    "visibility": b["visibility"], "sources": [_source_pin(s) for s in b.get("sources") or []],
                    **rec, "written": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
            path = BENCH_ROOT / b["id"] / "lock.json"
            path.write_text(json.dumps(lock, indent=2) + "\n")
            print(path)
    except BenchError as e:
        print(f"bench.py: {e}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
