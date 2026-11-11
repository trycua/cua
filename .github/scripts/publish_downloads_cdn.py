#!/usr/bin/env python3
"""Copy the assets of GitHub releases to the downloads bucket (S3 behind CloudFront).

GitHub Releases stays the source of truth. For every tag given, this copies each
asset to s3://<bucket>/<prefix><tag>/<asset> (the same path as
github.com/<repo>/releases/download/<tag>/<asset>), streaming: the bytes go from
GitHub through a pipe into `aws s3 cp -`, so a 345 MB DMG is never held on disk.

Checks, all fatal:
  * the streamed byte count equals the size GitHub reports for the asset;
  * the streamed sha256 equals GitHub's asset digest (or, for an old asset
    without one, the release's checksums file; with neither, size only, and the
    summary says so);
  * a versioned tag's existing object is never replaced by different bytes
    (use --allow-overwrite for a deliberate rebuild).

Order: all plain assets first, then the feeds and manifests (appcast,
latest*.yml, release-artifacts.json, nightly.json, install scripts) LAST, and a
feed is only uploaded when every file it names is already in the bucket. Then
the mutable paths are invalidated. Re-running is safe: an object that already
has the expected size and sha256 is left alone.

Only the standard library plus the `aws`, `gh` and `curl` command line tools.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from urllib.parse import quote, unquote

DEFAULT_BUCKET = "trycua-cua-downloads"
DEFAULT_REGION = "us-west-2"
DEFAULT_DISTRIBUTION = "E2OVJ13Z0CUD0S"
DEFAULT_CDN_HOST = "ds1hj2kk4nzuw.cloudfront.net"

ROLLING_TAGS = {"cua-install-latest", "cua-spaces-latest", "cua-spaces-nightly"}
CACHE_IMMUTABLE = "public, max-age=31536000, immutable"
CACHE_MUTABLE = "public, max-age=60"
CHUNK = 1024 * 1024

FEED_RE = re.compile(
    r"""^(
        .*appcast.*\.xml
      | (latest|beta|alpha|nightly)[\w.-]*\.ya?ml
      | .*-mac\.yml
      | (latest|nightly)\.json
      | release-artifacts\.json
      | (un)?install\.(sh|ps1)
    )$""",
    re.VERBOSE | re.IGNORECASE,
)
CHECKSUM_FILE_RE = re.compile(r"(^|[-_.])(sha256sums?|checksums?)(\.txt)?$|\.sha256$", re.IGNORECASE)
REF_RE = re.compile(r"github\.com/[\w.-]+/[\w.-]+/releases/download/([^/\s\"'<>]+)/([^\s\"'<>?#]+)")

CONTENT_TYPES = [
    (".dmg", "application/x-apple-diskimage"),
    (".pkg", "application/octet-stream"),
    (".tar.gz", "application/gzip"),
    (".tgz", "application/gzip"),
    (".tar.xz", "application/x-xz"),
    (".zip", "application/zip"),
    (".whl", "application/zip"),
    (".exe", "application/vnd.microsoft.portable-executable"),
    (".msi", "application/x-msi"),
    (".deb", "application/vnd.debian.binary-package"),
    (".rpm", "application/x-rpm"),
    (".appimage", "application/octet-stream"),
    (".blockmap", "application/octet-stream"),
    (".xml", "application/xml"),
    (".yml", "text/yaml; charset=utf-8"),
    (".yaml", "text/yaml; charset=utf-8"),
    (".json", "application/json"),
    (".bundle", "application/json"),
    (".sh", "text/x-shellscript; charset=utf-8"),
    (".ps1", "text/plain; charset=utf-8"),
    (".txt", "text/plain; charset=utf-8"),
    (".md", "text/plain; charset=utf-8"),
    (".sha256", "text/plain; charset=utf-8"),
]


class Fatal(Exception):
    pass


def is_rolling(tag: str) -> bool:
    return tag in ROLLING_TAGS or re.fullmatch(r".*-(latest|nightly)", tag) is not None


def is_feed(name: str) -> bool:
    return FEED_RE.match(name) is not None


def content_type(name: str) -> str:
    low = name.lower()
    if CHECKSUM_FILE_RE.search(low):
        return "text/plain; charset=utf-8"
    for suffix, ctype in CONTENT_TYPES:
        if low.endswith(suffix):
            return ctype
    return "application/octet-stream"


def feed_refs(text: str) -> set[tuple[str, str]]:
    """(tag, asset) pairs a feed or manifest names with absolute release URLs."""
    return {(unquote(t), unquote(a)) for t, a in REF_RE.findall(text)}


# --------------------------------------------------------------------------- sources


class GithubSource:
    def __init__(self, repo: str):
        self.repo = repo
        self.token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN") or ""

    def _gh(self, *args: str) -> str:
        out = subprocess.run(["gh", "api", *args], capture_output=True, text=True)
        if out.returncode != 0:
            raise Fatal(f"gh api {' '.join(args)} failed: {out.stderr.strip()[:300]}")
        return out.stdout

    def release(self, tag: str) -> dict:
        """The published release for a tag with all its assets. Drafts do not resolve."""
        try:
            rel = json.loads(self._gh(f"repos/{self.repo}/releases/tags/{quote(tag, safe='')}"))
        except Fatal as e:
            raise Fatal(f"no published release for tag {tag} ({e})") from None
        lines = self._gh("--paginate", f"repos/{self.repo}/releases/{rel['id']}/assets?per_page=100", "--jq", ".[]")
        assets = [json.loads(line) for line in lines.splitlines() if line.strip()]
        return {"id": rel["id"], "draft": rel.get("draft"), "prerelease": rel.get("prerelease"), "assets": assets}

    def stream(self, asset: dict):
        """Yield the asset's bytes. Raises if the transfer ends early or curl fails."""
        url = f"https://api.github.com/repos/{self.repo}/releases/assets/{asset['id']}"
        cmd = [
            "curl", "-sSL", "--fail", "--retry", "5", "--retry-all-errors", "--retry-delay", "3",
            "-H", "Accept: application/octet-stream", "-H", "X-GitHub-Api-Version: 2022-11-28",
        ]
        if self.token:
            cmd += ["-H", f"Authorization: Bearer {self.token}"]  # curl drops it on the redirect to the storage host
        p = subprocess.Popen(cmd + [url], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        assert p.stdout is not None
        try:
            while True:
                chunk = p.stdout.read(CHUNK)
                if not chunk:
                    break
                yield chunk
        finally:
            p.stdout.close()
            err = p.stderr.read().decode(errors="replace") if p.stderr else ""
            rc = p.wait()
        if rc != 0:
            raise Fatal(f"download of {asset['name']} failed (curl exit {rc}): {err.strip()[:300]}")

    def read(self, asset: dict) -> bytes:
        return b"".join(self.stream(asset))


# --------------------------------------------------------------------------- stores


class S3Store:
    """The bucket, through the aws CLI (credentials come from the environment)."""

    def __init__(self, bucket: str, region: str, distribution: str):
        self.bucket, self.region, self.distribution = bucket, region, distribution

    def _aws(self, *args: str, check=True, **kw) -> subprocess.CompletedProcess:
        p = subprocess.run(["aws", *args, "--region", self.region], capture_output=True, text=True, **kw)
        if check and p.returncode != 0:
            raise Fatal(f"aws {' '.join(args[:3])} failed: {p.stderr.strip()[:400]}")
        return p

    def head(self, key: str) -> dict | None:
        p = self._aws("s3api", "head-object", "--bucket", self.bucket, "--key", key, check=False)
        if p.returncode != 0:
            if re.search(r"\b404\b|Not Found|NoSuchKey", p.stderr):
                return None
            raise Fatal(f"head-object {key} failed: {p.stderr.strip()[:400]}")
        d = json.loads(p.stdout)
        return {
            "size": d.get("ContentLength"),
            "sha256": (d.get("Metadata") or {}).get("sha256"),
            "content_type": d.get("ContentType"),
            "cache_control": d.get("CacheControl"),
        }

    def put_stream(self, key, size, ctype, cache, meta, chunks) -> None:
        cmd = [
            "aws", "s3", "cp", "-", f"s3://{self.bucket}/{key}", "--region", self.region, "--only-show-errors",
            "--expected-size", str(max(size, 1)), "--content-type", ctype, "--cache-control", cache,
            "--metadata", ",".join(f"{k}={v}" for k, v in meta.items()),
        ]
        p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stderr=subprocess.PIPE)
        assert p.stdin is not None and p.stderr is not None
        failure: BaseException | None = None
        try:
            for c in chunks:
                p.stdin.write(c)
        except BaseException as e:  # noqa: BLE001 - report after aws is reaped
            failure = e
        finally:
            try:
                p.stdin.close()
            except BrokenPipeError:
                pass
            err = p.stderr.read().decode(errors="replace")
            rc = p.wait()
        if failure is not None:
            raise failure
        if rc != 0:
            raise Fatal(f"aws s3 cp to {key} failed: {err.strip()[:400]}")

    def relabel(self, key, ctype, cache, meta) -> None:
        """Replace an object's metadata in place (sha256 learned or marked bad)."""
        src = quote(f"{self.bucket}/{key}", safe="/")
        self._aws(
            "s3api", "copy-object", "--bucket", self.bucket, "--key", key, "--copy-source", src,
            "--metadata-directive", "REPLACE", "--content-type", ctype, "--cache-control", cache,
            "--metadata", ",".join(f"{k}={v}" for k, v in meta.items()),
        )

    def invalidate(self, paths: list[str]) -> str:
        out = []
        for i in range(0, len(paths), 500):
            p = self._aws(
                "cloudfront", "create-invalidation", "--distribution-id", self.distribution,
                "--paths", *paths[i : i + 500], "--query", "Invalidation.Id", "--output", "text",
            )
            out.append(p.stdout.strip())
        return ",".join(out)


class DryRunStore:
    """Looks at the public CDN (no credentials) and changes nothing."""

    def __init__(self, cdn_host: str):
        self.cdn_host = cdn_host

    def head(self, key: str) -> dict | None:
        req = urllib.request.Request(f"https://{self.cdn_host}/{quote(key)}", method="HEAD")
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return {"size": int(r.headers.get("Content-Length", -1)), "sha256": None,
                        "content_type": r.headers.get("Content-Type"), "cache_control": r.headers.get("Cache-Control")}
        except urllib.error.HTTPError as e:
            if e.code in (403, 404):
                return None
            raise Fatal(f"HEAD {key} -> {e.code}") from None

    def put_stream(self, *a, **k):  # pragma: no cover - never reached
        raise Fatal("dry run must not upload")

    relabel = invalidate = put_stream


# --------------------------------------------------------------------------- the copy


class Result:
    def __init__(self, tag: str):
        self.tag = tag
        self.rows: list[dict] = []
        self.errors: list[str] = []
        self.invalidate: list[str] = []

    def add(self, name: str, status: str, size: int = 0, note: str = ""):
        self.rows.append({"name": name, "status": status, "size": size, "note": note})

    def fail(self, msg: str):
        self.errors.append(msg)
        print(f"::error::{msg}", flush=True)


def expected_sha(asset: dict, checksums: dict[str, str]) -> tuple[str | None, str]:
    d = asset.get("digest") or ""
    if d.startswith("sha256:"):
        return d[7:].lower(), "github-digest"
    if asset["name"] in checksums:
        return checksums[asset["name"]], "checksums-file"
    return None, "none"


def parse_checksums(text: str) -> dict[str, str]:
    out = {}
    for line in text.splitlines():
        m = re.match(r"^([0-9a-fA-F]{64})\s+\*?(.+?)\s*$", line)
        if m:
            out[os.path.basename(m.group(2))] = m.group(1).lower()
    return out


class Publisher:
    def __init__(self, src, store, args):
        self.src, self.store, self.args = src, store, args
        self.done_tags: dict[str, dict] = {}
        self.releases: dict[str, dict] = {}

    def key(self, tag: str, name: str) -> str:
        return f"{self.args.prefix}{tag}/{name}"

    def release(self, tag: str) -> dict:
        if tag not in self.releases:
            self.releases[tag] = self.src.release(tag)
        return self.releases[tag]

    def verified_present(self, tag: str, name: str) -> bool:
        h = self.store.head(self.key(tag, name))
        return bool(h and h["size"] and h["sha256"] and not str(h["sha256"]).startswith("BAD:"))

    def checksums_for(self, rel: dict) -> dict[str, str]:
        sums: dict[str, str] = {}
        for a in rel["assets"]:
            if CHECKSUM_FILE_RE.search(a["name"]) and a["size"] < 5_000_000:
                try:
                    sums.update(parse_checksums(self.src.read(a).decode(errors="replace")))
                except Fatal:
                    pass
        return sums

    def copy_one(self, tag: str, asset: dict, sums: dict[str, str], res: Result) -> bool:
        name, size = asset["name"], asset["size"]
        key = self.key(tag, name)
        want, how = expected_sha(asset, sums)
        ctype, cache = content_type(name), CACHE_MUTABLE if is_rolling(tag) else CACHE_IMMUTABLE
        existing = self.store.head(key)
        if existing and existing["size"] == size and (
            (want and existing["sha256"] == want) or (not want and existing["sha256"] not in (None, ""))
        ):
            res.add(name, "present", size)
            return True
        if self.args.dry_run:
            if existing and existing["size"] == size:
                res.add(name, "would-skip (same size on the CDN; hash checked on a real run)", size)
            else:
                res.add(name, "would-copy" + (" (replaces a different object)" if existing else ""), size)
            return True
        if existing and not is_rolling(tag) and not self.args.allow_overwrite:
            if not str(existing["sha256"] or "").startswith("BAD:"):
                res.fail(f"{key}: the CDN already has a different object at this versioned path "
                         f"(size {existing['size']} vs {size}); not overwriting. Use allow_overwrite for a deliberate rebuild")
                res.add(name, "FAILED (immutable object differs)", size)
                return False
        last = ""
        for attempt in range(1, 4):
            h, n = hashlib.sha256(), 0

            def gen():
                nonlocal n
                for c in self.src.stream(asset):
                    h.update(c)
                    n += len(c)
                    yield c

            try:
                self.store.put_stream(key, size, ctype, cache, {"sha256": want or "unverified", "gh-asset-id": str(asset["id"]),
                                                              "source-tag": tag}, gen())
                got = h.hexdigest()
                if n != size:
                    raise Fatal(f"{key}: copied {n} bytes, GitHub says the asset is {size}")
                if want and got != want:
                    raise Fatal(f"{key}: sha256 {got} does not match {how} {want}")
                if not want:
                    self.store.relabel(key, ctype, cache, {"sha256": got, "gh-asset-id": str(asset["id"]), "source-tag": tag})
                after = self.store.head(key)
                if not after or after["size"] != size:
                    raise Fatal(f"{key}: after the copy the bucket has size {after and after['size']}, expected {size}")
                res.add(name, "copied" + ("" if want else " (size only: GitHub has no digest or checksum for this asset)"), size)
                if is_rolling(tag) or existing:
                    res.invalidate.append("/" + key)
                return True
            except Exception as e:  # noqa: BLE001
                last = str(e)
                try:  # the object may be partial or wrong: mark it so no run counts it as present
                    self.store.relabel(key, ctype, cache, {"sha256": "BAD:" + (h.hexdigest() if n else "none"), "source-tag": tag})
                except Exception:  # noqa: BLE001
                    pass
                print(f"attempt {attempt} for {key} failed: {last}", flush=True)
                time.sleep(2 * attempt)
        res.fail(f"{key}: {last}")
        res.add(name, "FAILED", size, last)
        return False

    def gate_feed(self, tag: str, name: str, data: bytes, rel_assets: set[str], res: Result, follow: bool = True) -> str | None:
        """None when every file the feed names is in the bucket; else why not."""
        text = data.decode(errors="replace")
        missing = []
        for rtag, rname in sorted(feed_refs(text)):
            if rtag == tag and rname not in rel_assets:
                missing.append(f"{rtag}/{rname} (not on the GitHub release)")
                continue
            if follow and rtag != tag and rtag not in self.done_tags and not is_rolling(rtag) and not self.args.no_follow:
                self.follow(rtag)
            if self.args.dry_run:
                continue
            if not self.verified_present(rtag, rname):
                missing.append(f"{rtag}/{rname}")
        if missing:
            return f"{name} names files that are not in the bucket yet: " + ", ".join(missing[:8]) + (" ..." if len(missing) > 8 else "")
        return None

    def follow(self, tag: str) -> None:
        print(f"following a reference to {tag}", flush=True)
        try:
            r = self.publish_tag(tag, follow=False)  # one level only: a followed tag's own feeds do not pull in more tags
            self.results.append(r)
        except Fatal as e:
            print(f"::warning::could not mirror referenced tag {tag}: {e}", flush=True)

    def publish_tag(self, tag: str, follow: bool = True) -> Result:
        res = Result(tag)
        self.done_tags[tag] = {}
        rel = self.release(tag)
        assets = rel["assets"]
        sums = self.checksums_for(rel)
        plain = [a for a in assets if not is_feed(a["name"])]
        feeds = [a for a in assets if is_feed(a["name"])]
        plain.sort(key=lambda a: -a["size"])
        with concurrent.futures.ThreadPoolExecutor(max_workers=self.args.jobs) as ex:
            oks = list(ex.map(lambda a: self.copy_one(tag, a, sums, res), plain))
        if not all(oks):
            for a in feeds:
                res.add(a["name"], "HELD BACK (an asset of this release failed)", a["size"])
            res.fail(f"{tag}: some assets failed, so no feed or manifest of this release was uploaded")
            return res
        names = {a["name"] for a in assets}
        for a in feeds:
            try:
                data = self.src.read(a)
            except Fatal as e:
                res.fail(str(e))
                res.add(a["name"], "FAILED", a["size"])
                continue
            want, _ = expected_sha(a, sums)
            if len(data) != a["size"] or (want and hashlib.sha256(data).hexdigest() != want):
                res.fail(f"{tag}/{a['name']}: downloaded feed does not match GitHub's size/digest")
                res.add(a["name"], "FAILED", a["size"])
                continue
            why = self.gate_feed(tag, a["name"], data, names, res, follow)
            if why:
                if follow and not self.args.no_follow:
                    res.fail(f"{tag}: {why}")
                else:  # tags a feed names were deliberately not copied (--no-follow, or reached only by following): warn, copy those tags to release the feed
                    print(f"::warning::{tag}: {why}", flush=True)
                res.add(a["name"], "HELD BACK (references files of tags not copied)", a["size"])
                continue
            ok = self.copy_one_bytes(tag, a, data, res)
            if ok and not self.args.dry_run:
                res.invalidate.append("/" + self.key(tag, a["name"]))
        return res

    def copy_one_bytes(self, tag, asset, data: bytes, res: Result) -> bool:
        class Mem:
            def stream(_, a):
                yield data

        saved, self.src = self.src, Mem()
        try:
            return self.copy_one(tag, asset, {}, res)
        finally:
            self.src = saved

    def run(self, tags: list[str]) -> list[Result]:
        self.results: list[Result] = []
        for tag in tags:
            if tag in self.done_tags:
                continue
            try:
                self.results.append(self.publish_tag(tag))
            except Fatal as e:
                r = Result(tag)
                r.fail(str(e))
                self.results.append(r)
        return self.results


def write_summary(results: list[Result], args, invalidation: str) -> str:
    lines = [f"## Downloads CDN {'dry run' if args.dry_run else 'publish'}", ""]
    bad = sum(len(r.errors) for r in results)
    if bad:
        lines += [f"**FAILED: {bad} problem(s). GitHub is unaffected and stays the source of truth; the CDN copy is incomplete.**", ""]
    else:
        lines += ["All requested assets are on the CDN and match GitHub." if not args.dry_run else "Nothing was changed.", ""]
    for r in results:
        total = sum(x["size"] for x in r.rows)
        counts: dict[str, int] = {}
        for x in r.rows:
            counts[x["status"].split(" ")[0]] = counts.get(x["status"].split(" ")[0], 0) + 1
        lines.append(f"### {r.tag} ({len(r.rows)} files, {total / 1e6:.1f} MB) " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
        lines += ["", "| file | status | MB |", "|---|---|---|"]
        for x in r.rows:
            lines.append(f"| {x['name']} | {x['status']} | {x['size'] / 1e6:.1f} |")
        for e in r.errors:
            lines.append(f"\n- ERROR: {e}")
        lines.append("")
    if invalidation:
        lines.append(f"Invalidation: {invalidation}")
    text = "\n".join(lines)
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a") as f:
            f.write(text + "\n")
    return text


def main(argv=None, src=None, store=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", "trycua/cua"))
    ap.add_argument("--tags", required=True, help="release tags (comma, space or newline separated), copied in this order")
    ap.add_argument("--bucket", default=DEFAULT_BUCKET)
    ap.add_argument("--prefix", default="", help="key prefix, e.g. test/ for a rehearsal")
    ap.add_argument("--region", default=DEFAULT_REGION)
    ap.add_argument("--distribution", default=DEFAULT_DISTRIBUTION)
    ap.add_argument("--cdn-host", default=DEFAULT_CDN_HOST)
    ap.add_argument("--dry-run", action="store_true", help="list what would be copied; changes nothing, needs no AWS credentials")
    ap.add_argument("--allow-overwrite", action="store_true", help="let different bytes replace a versioned object (a deliberate rebuild)")
    ap.add_argument("--no-follow", action="store_true", help="do not copy tags that a feed references but that are not in --tags; such a feed is held back with a warning")
    ap.add_argument("--jobs", type=int, default=4)
    args = ap.parse_args(argv)
    tags = [t for t in re.split(r"[\s,]+", args.tags) if t]
    if not tags:
        ap.error("no tags")
    if args.prefix and not args.prefix.endswith("/"):
        args.prefix += "/"
    src = src or GithubSource(args.repo)
    store = store or (DryRunStore(args.cdn_host) if args.dry_run else S3Store(args.bucket, args.region, args.distribution))
    pub = Publisher(src, store, args)
    results = pub.run(tags)
    paths = sorted({p for r in results for p in r.invalidate})
    inval = ""
    if paths and not args.dry_run:
        try:
            inval = store.invalidate(paths)
            print(f"invalidated {len(paths)} path(s): {inval}")
        except Fatal as e:
            results[-1].fail(f"invalidation failed: {e}")
    print(write_summary(results, args, inval))
    return 1 if any(r.errors for r in results) else 0


if __name__ == "__main__":
    sys.exit(main())
