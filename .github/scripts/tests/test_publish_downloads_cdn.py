"""publish_downloads_cdn.py with a fake GitHub and a fake bucket (no network, no AWS)."""
import hashlib
import importlib.util
import sys
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]


def _load(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f"{name}.py")
    m = importlib.util.module_from_spec(spec)
    sys.modules[name] = m
    spec.loader.exec_module(m)
    return m


cdn = _load("publish_downloads_cdn")
plan = _load("backfill_downloads_plan")


def sha(b):
    return hashlib.sha256(b).hexdigest()


class FakeSource:
    def __init__(self, releases, corrupt=()):
        self.rel, self.corrupt, self.data = releases, set(corrupt), {}
        self.n = 0

    def add(self, tag, files):
        assets = []
        for name, body in files.items():
            self.n += 1
            self.data[self.n] = body
            assets.append({"id": self.n, "name": name, "size": len(body), "digest": "sha256:" + sha(body)})
        self.rel[tag] = {"id": len(self.rel) + 1, "assets": assets}

    def release(self, tag):
        if tag not in self.rel:
            raise cdn.Fatal(f"no published release for tag {tag}")
        return self.rel[tag]

    def stream(self, asset):
        b = self.data[asset["id"]]
        if asset["name"] in self.corrupt:
            b = b[:-1] + b"X"
        yield b[: len(b) // 2]
        yield b[len(b) // 2 :]

    def read(self, asset):
        return b"".join(self.stream(asset))


class FakeStore:
    def __init__(self):
        self.objs, self.inval, self.log = {}, [], []

    def head(self, key):
        o = self.objs.get(key)
        return None if o is None else {"size": len(o["body"]), "sha256": o["meta"].get("sha256"),
                                       "content_type": o["ct"], "cache_control": o["cc"]}

    def put_stream(self, key, size, ct, cc, meta, chunks):
        self.objs[key] = {"body": b"".join(chunks), "ct": ct, "cc": cc, "meta": dict(meta)}
        self.log.append(key)

    def relabel(self, key, ct, cc, meta):
        self.objs[key].update(ct=ct, cc=cc, meta=dict(meta))

    def invalidate(self, paths):
        self.inval += paths
        return "I1"


def run(src, store, tags, *extra):
    return cdn.main(["--repo", "o/r", "--tags", ",".join(tags), *extra], src=src, store=store)


def test_classifiers():
    assert cdn.is_rolling("cua-spaces-latest") and cdn.is_rolling("cua-spaces-nightly")
    assert not cdn.is_rolling("cua-spaces-v0.22.3")
    assert not cdn.is_rolling("nightly-cua-driver-rs-v0.45.2-nightly.20261109.1")
    for n in ["cua-spaces-appcast.xml", "cua-spaces-nightly-appcast.xml", "latest.yml", "latest-linux-arm64.yml",
              "beta-mac.yml", "nightly-linux.yml", "nightly.json", "release-artifacts.json", "install.sh", "uninstall.ps1", "latest.json"]:
        assert cdn.is_feed(n), n
    for n in ["Cua-Spaces-0.22.3.dmg", "checksums.txt", "x.AppImage.blockmap", "cua-spaces-0.22.3-darwin-universal.dmg"]:
        assert not cdn.is_feed(n), n
    assert cdn.content_type("a.dmg") == "application/x-apple-diskimage"
    assert cdn.content_type("cua-spaces-appcast.xml") == "application/xml"
    assert cdn.content_type("install.sh").startswith("text/x-shellscript")
    assert cdn.content_type("SHA256SUMS").startswith("text/plain")
    assert cdn.content_type("a.tar.gz") == "application/gzip"


def test_feed_refs():
    t = 'url: https://github.com/trycua/cua/releases/download/cua-spaces-v1.2.3/Cua%20Spaces.exe\n<enclosure url="https://github.com/trycua/cua/releases/download/cua-sdk-v0.1.0/a.tgz?x=1"/>'
    assert cdn.feed_refs(t) == {("cua-spaces-v1.2.3", "Cua Spaces.exe"), ("cua-sdk-v0.1.0", "a.tgz")}


def test_copy_verify_feeds_last_and_idempotent():
    src, store = FakeSource({}), FakeStore()
    src.add("app-v1", {"a.dmg": b"A" * 100, "b.exe": b"B" * 50})
    feed = b'<x url="https://github.com/o/r/releases/download/app-v1/a.dmg"/>'
    src.add("app-latest", {"appcast.xml": feed, "notes.txt": b"hi"})
    assert run(src, store, ["app-v1", "app-latest"]) == 0
    assert store.objs["app-v1/a.dmg"]["cc"] == cdn.CACHE_IMMUTABLE
    assert store.objs["app-latest/appcast.xml"]["cc"] == cdn.CACHE_MUTABLE
    assert store.objs["app-latest/appcast.xml"]["ct"] == "application/xml"
    assert store.log.index("app-latest/notes.txt") < store.log.index("app-latest/appcast.xml")
    assert "/app-latest/appcast.xml" in store.inval
    n = len(store.log)
    assert run(src, store, ["app-v1", "app-latest"]) == 0
    assert len(store.log) == n, "a re-run must not copy again"


def test_feed_held_back_when_referenced_file_missing():
    src, store = FakeSource({}), FakeStore()
    src.add("app-latest", {"appcast.xml": b"https://github.com/o/r/releases/download/gone-v9/z.dmg"})
    assert run(src, store, ["app-latest"]) == 1
    assert "app-latest/appcast.xml" not in store.objs


def test_feed_follows_reference_and_copies_the_target_first():
    src, store = FakeSource({}), FakeStore()
    src.add("app-v1", {"a.dmg": b"A" * 10})
    src.add("app-latest", {"appcast.xml": b"https://github.com/o/r/releases/download/app-v1/a.dmg"})
    assert run(src, store, ["app-latest"]) == 0
    assert store.log.index("app-v1/a.dmg") < store.log.index("app-latest/appcast.xml")


def test_hash_mismatch_fails_marks_bad_and_holds_feeds():
    src, store = FakeSource({}, corrupt={"a.dmg"}), FakeStore()
    src.add("app-v1", {"a.dmg": b"A" * 10, "appcast.xml": b"<x/>"})
    assert run(src, store, ["app-v1"]) == 1
    assert store.objs["app-v1/a.dmg"]["meta"]["sha256"].startswith("BAD:")
    assert "app-v1/appcast.xml" not in store.objs
    # the source is fixed: a bad object may be replaced even on a versioned path
    src.corrupt.clear()
    assert run(src, store, ["app-v1"]) == 0
    assert store.objs["app-v1/a.dmg"]["body"] == b"A" * 10


def test_versioned_object_is_never_replaced_by_different_bytes():
    src, store = FakeSource({}), FakeStore()
    src.add("app-v1", {"a.dmg": b"A" * 10})
    assert run(src, store, ["app-v1"]) == 0
    src.add("app-v1", {"a.dmg": b"Z" * 10})
    assert run(src, store, ["app-v1"]) == 1
    assert store.objs["app-v1/a.dmg"]["body"] == b"A" * 10
    assert run(src, store, ["app-v1"], "--allow-overwrite") == 0
    assert store.objs["app-v1/a.dmg"]["body"] == b"Z" * 10


def test_rolling_asset_is_replaced_and_invalidated():
    src, store = FakeSource({}), FakeStore()
    src.add("app-nightly", {"app-nightly.dmg": b"1" * 10})
    assert run(src, store, ["app-nightly"]) == 0
    src.add("app-nightly", {"app-nightly.dmg": b"2" * 10})
    store.inval.clear()
    assert run(src, store, ["app-nightly"]) == 0
    assert store.objs["app-nightly/app-nightly.dmg"]["body"] == b"2" * 10
    assert store.inval == ["/app-nightly/app-nightly.dmg"]


def test_prefix_and_missing_release():
    src, store = FakeSource({}), FakeStore()
    src.add("app-v1", {"a.dmg": b"A"})
    assert run(src, store, ["app-v1"], "--prefix", "test") == 0
    assert "test/app-v1/a.dmg" in store.objs
    assert run(src, store, ["nope-v1"]) == 1


def test_checksums_file_fallback():
    src, store = FakeSource({}), FakeStore()
    body = b"Q" * 20
    src.add("app-v1", {"a.bin": body, "checksums.txt": f"{sha(body)}  a.bin\n".encode()})
    for a in src.rel["app-v1"]["assets"]:
        if a["name"] == "a.bin":
            a["digest"] = None
    assert run(src, store, ["app-v1"]) == 0
    assert store.objs["app-v1/a.bin"]["meta"]["sha256"] == sha(body)


def test_plan_picks_newest_of_each_product_and_rolling_last():
    rels = [{"tag_name": t, "draft": d} for t, d in [
        ("cua-spaces-v0.22.3", False), ("cua-spaces-v0.25.2", True), ("cua-spaces-v0.22.5", False), ("cua-spaces-v0.22.5-beta.1", False),
        ("cua-sdk-v0.17.1", False), ("cua-sdk-v0.9.0", False), ("cua-spacesd-v0.12.8", False), ("cua-driver-rs-v0.45.4", False), ("lume-v1.0.0", False)]]
    out = plan.pick(rels, 2)
    assert out[:2] == ["cua-spaces-v0.22.5", "cua-spaces-v0.22.3"]
    assert {"cua-sdk-v0.17.1", "cua-spacesd-v0.12.8", "cua-driver-rs-v0.45.4"} <= set(out)
    assert out[-3:] == plan.ROLLING
    assert "cua-sdk-v0.9.0" not in out and "lume-v1.0.0" not in out


def test_followed_tag_does_not_pull_in_more_tags():
    src, store = FakeSource({}), FakeStore()
    src.add("old-v0", {"z.dmg": b"Z"})
    src.add("app-v1", {"a.dmg": b"A", "release-artifacts.json": b"https://github.com/o/r/releases/download/old-v0/z.dmg"})
    src.add("app-latest", {"appcast.xml": b"https://github.com/o/r/releases/download/app-v1/a.dmg"})
    assert run(src, store, ["app-latest"]) == 0
    assert "app-v1/a.dmg" in store.objs and "app-latest/appcast.xml" in store.objs
    assert "old-v0/z.dmg" not in store.objs and "app-v1/release-artifacts.json" not in store.objs
    # asking for the tag itself completes it
    assert run(src, store, ["app-v1"]) == 0
    assert "app-v1/release-artifacts.json" in store.objs


def test_no_follow_holds_the_feed_with_a_warning_not_an_error():
    src, store = FakeSource({}), FakeStore()
    src.add("app-v1", {"a.dmg": b"A", "release-artifacts.json": b"https://github.com/o/r/releases/download/other-v1/z.dmg"})
    assert run(src, store, ["app-v1"], "--no-follow") == 0
    assert "app-v1/a.dmg" in store.objs and "app-v1/release-artifacts.json" not in store.objs


def test_plan_without_rolling():
    rels = [{"tag_name": "cua-spaces-v1.0.0", "draft": False}, {"tag_name": "cua-sdk-v2.0.0", "draft": False}]
    assert plan.pick(rels, 1, include_rolling=False) == ["cua-spaces-v1.0.0", "cua-sdk-v2.0.0"]
