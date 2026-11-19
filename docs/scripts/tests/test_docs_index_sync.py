"""Offline tests for the bounded S3 docs generation consumer, using a fake S3 client."""

import fcntl
import hashlib
import io
import json
import os
import sys
import threading
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docs-mcp-server"))
import docs_index as index  # noqa: E402
import sync_docs_index as sync_mod  # noqa: E402

BUCKET = "docs-bucket"


class ClientError(Exception):
    def __init__(self, code):
        super().__init__(code)
        self.response = {"Error": {"Code": code}}


class FakeS3:
    def __init__(self, objects):
        self.objects = dict(objects)
        self.requests = []
        self.on_download = None

    def get_object(self, *, Bucket, Key):
        assert Bucket == BUCKET
        self.requests.append(("get", Key))
        if Key not in self.objects:
            raise ClientError("NoSuchKey")
        body = self.objects[Key]
        return {"Body": io.BytesIO(body), "ContentLength": len(body)}

    def download_file(self, Bucket, Key, Filename):
        assert Bucket == BUCKET
        self.requests.append(("download", Key))
        if self.on_download and self.on_download(self, Key, Filename):
            return
        if Key not in self.objects:
            raise ClientError("404")
        Path(Filename).write_bytes(self.objects[Key])

    def downloaded(self):
        return [key for kind, key in self.requests if kind == "download"]


def sha(data):
    return hashlib.sha256(data).hexdigest()


def make_generation(build_id="a" * 32, salt=b""):
    files = {
        "docs.sqlite": b"sqlite-bytes" + salt,
        "docs.lance/data/chunk.lance": b"lance-bytes" + salt,
        "docs.lance/_versions/1.manifest": b"version-bytes",
    }
    manifest = {
        "format_version": index.FORMAT_VERSION,
        "build_id": build_id,
        "corpus_id": "c" * 64,
        "source": "https://cua.ai/docs",
        "captured_at": "2026-09-04T06:00:00+00:00",
        "embedding": index.EMBEDDING,
        "writer_versions": index.WRITER_VERSIONS,
        "expected_urls": ["https://cua.ai/docs/a"],
        "included_urls": ["https://cua.ai/docs/a"],
        "exclusions": [],
        "chunk_count": 1,
        "files": {name: sha(data) for name, data in files.items()},
    }
    return manifest, files


def publish(manifest, files, objects=None, pointer=True):
    """Raw manifest bytes are deliberately non-canonical; the pointer hashes the object."""
    objects = objects if objects is not None else {}
    prefix = f"docs_db/generations/{manifest['build_id']}/"
    for name, data in files.items():
        objects[prefix + name] = data
    objects[prefix + "manifest.json"] = json.dumps(manifest, indent=3).encode()
    if pointer:
        objects["docs_db/current.json"] = json.dumps(index.pointer_for(manifest)).encode()
    return objects


def local_pointer(root):
    file = Path(root) / "current.json"
    return json.loads(file.read_text()) if file.exists() else None


def snapshot(root):
    return {
        p.relative_to(root).as_posix(): p.read_bytes()
        for p in Path(root).rglob("*")
        if p.is_file() and p.name != sync_mod.LOCK_NAME
    }


def seed_legacy(root):
    root = Path(root)
    (root / "docs.sqlite").write_bytes(b"legacy-sqlite")
    (root / "docs.lance").mkdir()
    (root / "docs.lance/legacy.lance").write_bytes(b"legacy-lance")
    (root / "code_index.sqlite").write_bytes(b"code-index")
    old, old_files = make_generation("0" * 32, salt=b"old")
    target = root / "generations" / old["build_id"]
    for name, data in old_files.items():
        (target / name).parent.mkdir(parents=True, exist_ok=True)
        (target / name).write_bytes(data)
    index.write_json(target / "manifest.json", old)
    index.activate(root, old)
    return old


def test_complete_install_downloads_only_allowlisted_keys(tmp_path):
    manifest, files = make_generation()
    s3 = FakeS3(publish(manifest, files))
    result = sync_mod.sync(s3, BUCKET, tmp_path)
    assert result == {"status": "installed", "build_id": manifest["build_id"], "files": 3}
    assert local_pointer(tmp_path) == index.pointer_for(manifest)
    target = tmp_path / "generations" / manifest["build_id"]
    index.verify_files(target, manifest)
    assert json.loads((target / "manifest.json").read_text()) == manifest
    prefix = f"docs_db/generations/{manifest['build_id']}/"
    assert sorted(s3.downloaded()) == sorted(prefix + name for name in files)
    assert [k for kind, k in s3.requests if kind == "get"] == [
        "docs_db/current.json",
        prefix + "manifest.json",
    ]
    assert not [p for p in (tmp_path / "generations").iterdir() if p.name.startswith(".stage-")]


def test_idempotent_rerun_downloads_nothing(tmp_path):
    manifest, files = make_generation()
    s3 = FakeS3(publish(manifest, files))
    sync_mod.sync(s3, BUCKET, tmp_path)
    before = snapshot(tmp_path)
    s3.requests.clear()
    assert sync_mod.sync(s3, BUCKET, tmp_path)["status"] == "current"
    assert s3.downloaded() == []
    assert snapshot(tmp_path) == before


def test_interrupted_download_preserves_pointer_and_retries(tmp_path):
    old = seed_legacy(tmp_path)
    manifest, files = make_generation()
    s3 = FakeS3(publish(manifest, files))
    before = snapshot(tmp_path)

    def fail_second(store, key, filename):
        if len(store.downloaded()) == 2:
            raise OSError("Synthetic interrupted download")

    s3.on_download = fail_second
    with pytest.raises(sync_mod.SyncError, match="Download failed"):
        sync_mod.sync(s3, BUCKET, tmp_path)
    assert local_pointer(tmp_path) == index.pointer_for(old)
    assert not (tmp_path / "generations" / manifest["build_id"]).exists()
    assert snapshot(tmp_path) == before
    s3.on_download = None
    assert sync_mod.sync(s3, BUCKET, tmp_path)["status"] == "installed"
    assert local_pointer(tmp_path) == index.pointer_for(manifest)
    assert (tmp_path / "docs.sqlite").read_bytes() == b"legacy-sqlite"
    index.verify_files(tmp_path / "generations" / old["build_id"], old)


def test_manifest_hash_mismatch_fails_before_download(tmp_path):
    manifest, files = make_generation()
    objects = publish(manifest, files)
    tampered = {**manifest, "chunk_count": 99}
    objects[f"docs_db/generations/{manifest['build_id']}/manifest.json"] = json.dumps(
        tampered
    ).encode()
    s3 = FakeS3(objects)
    with pytest.raises(ValueError, match="Manifest identity mismatch"):
        sync_mod.sync(s3, BUCKET, tmp_path)
    assert s3.downloaded() == []
    assert local_pointer(tmp_path) is None


def test_file_hash_mismatch_fails_without_activation(tmp_path):
    manifest, files = make_generation()
    objects = publish(manifest, files)
    objects[f"docs_db/generations/{manifest['build_id']}/docs.sqlite"] = b"corrupt"
    with pytest.raises(ValueError, match="docs.sqlite"):
        sync_mod.sync(FakeS3(objects), BUCKET, tmp_path)
    assert local_pointer(tmp_path) is None
    assert list((tmp_path / "generations").iterdir()) == []


@pytest.mark.parametrize(
    "pointer",
    [
        {"build_id": "A" * 32, "manifest_sha256": "b" * 64},
        {"build_id": "../" + "a" * 29, "manifest_sha256": "b" * 64},
        {"build_id": "a" * 32, "manifest_sha256": "b" * 63},
        {"build_id": "a" * 32},
        {"build_id": "a" * 32, "manifest_sha256": "b" * 64, "extra": 1},
        ["a" * 32],
    ],
)
def test_malformed_pointer_rejected(tmp_path, pointer):
    s3 = FakeS3({"docs_db/current.json": json.dumps(pointer).encode()})
    with pytest.raises(ValueError, match="Invalid docs|not a JSON object"):
        sync_mod.sync(s3, BUCKET, tmp_path)
    assert len(s3.requests) == 1


def test_oversized_pointer_rejected(tmp_path):
    s3 = FakeS3({"docs_db/current.json": b" " * (sync_mod.MAX_POINTER_BYTES + 1)})
    with pytest.raises(ValueError, match="exceeds"):
        sync_mod.sync(s3, BUCKET, tmp_path)


@pytest.mark.parametrize(
    "name",
    [
        "../escape",
        "/etc/passwd",
        "docs.lance\\evil",
        "docs.lance//x",
        "docs.lance/./x",
        "docs.lance/../x",
        "docs.lance/",
        "docs.lance",
        "manifest.json",
        "other.bin",
        "",
    ],
)
def test_unsafe_manifest_paths_rejected_before_download(tmp_path, name):
    manifest, files = make_generation()
    manifest["files"][name] = "d" * 64
    s3 = FakeS3(publish(manifest, files))
    with pytest.raises(ValueError):
        sync_mod.sync(s3, BUCKET, tmp_path)
    assert s3.downloaded() == []


def test_malformed_digest_rejected(tmp_path):
    manifest, files = make_generation()
    manifest["files"]["docs.sqlite"] = "not-a-digest"
    s3 = FakeS3(publish(manifest, files))
    with pytest.raises(ValueError, match="Malformed digest"):
        sync_mod.sync(s3, BUCKET, tmp_path)
    assert s3.downloaded() == []


def test_symlink_in_download_rejected(tmp_path):
    manifest, files = make_generation()
    s3 = FakeS3(publish(manifest, files))
    outside = tmp_path / "outside"
    outside.write_bytes(files["docs.sqlite"])

    def link_instead(store, key, filename):
        if key.endswith("docs.sqlite"):
            os.symlink(outside, filename)
            return True

    s3.on_download = link_instead
    with pytest.raises(ValueError, match="Symlink"):
        sync_mod.sync(s3, BUCKET, tmp_path)
    assert local_pointer(tmp_path) is None
    assert list((tmp_path / "generations").iterdir()) == []


def test_existing_generation_conflict_is_not_altered(tmp_path):
    manifest, files = make_generation()
    s3 = FakeS3(publish(manifest, files))
    target = tmp_path / "generations" / manifest["build_id"]
    other, other_files = make_generation(manifest["build_id"], salt=b"different")
    for name, data in other_files.items():
        (target / name).parent.mkdir(parents=True, exist_ok=True)
        (target / name).write_bytes(data)
    index.write_json(target / "manifest.json", other)
    before = snapshot(tmp_path)
    with pytest.raises(ValueError, match="conflicts"):
        sync_mod.sync(s3, BUCKET, tmp_path)
    assert snapshot(tmp_path) == before
    assert s3.downloaded() == []
    assert local_pointer(tmp_path) is None


def test_existing_corrupt_generation_fails_closed(tmp_path):
    manifest, files = make_generation()
    s3 = FakeS3(publish(manifest, files))
    target = tmp_path / "generations" / manifest["build_id"]
    for name, data in files.items():
        (target / name).parent.mkdir(parents=True, exist_ok=True)
        (target / name).write_bytes(data)
    (target / "docs.sqlite").write_bytes(b"bit-rot")
    index.write_json(target / "manifest.json", manifest)
    before = snapshot(tmp_path)
    with pytest.raises(ValueError, match="docs.sqlite"):
        sync_mod.sync(s3, BUCKET, tmp_path)
    assert snapshot(tmp_path) == before
    assert local_pointer(tmp_path) is None


def test_existing_complete_generation_is_activated_without_download(tmp_path):
    manifest, files = make_generation()
    s3 = FakeS3(publish(manifest, files))
    target = tmp_path / "generations" / manifest["build_id"]
    for name, data in files.items():
        (target / name).parent.mkdir(parents=True, exist_ok=True)
        (target / name).write_bytes(data)
    index.write_json(target / "manifest.json", manifest)
    assert sync_mod.sync(s3, BUCKET, tmp_path)["status"] == "activated"
    assert s3.downloaded() == []
    assert local_pointer(tmp_path) == index.pointer_for(manifest)


def test_source_pointer_change_during_copy_installs_chosen_generation(tmp_path):
    first, first_files = make_generation("1" * 32)
    second, second_files = make_generation("2" * 32, salt=b"second")
    s3 = FakeS3(publish(first, first_files))

    def republish(store, key, filename):
        publish(second, second_files, store.objects)

    s3.on_download = republish
    result = sync_mod.sync(s3, BUCKET, tmp_path)
    assert result["build_id"] == first["build_id"]
    assert local_pointer(tmp_path) == index.pointer_for(first)
    assert all(first["build_id"] in key for key in s3.downloaded())
    assert not (tmp_path / "generations" / second["build_id"]).exists()
    assert sync_mod.sync(s3, BUCKET, tmp_path)["build_id"] == second["build_id"]
    index.verify_files(tmp_path / "generations" / first["build_id"], first)


def test_legacy_files_code_data_and_old_generation_untouched(tmp_path):
    old = seed_legacy(tmp_path)
    before = snapshot(tmp_path)
    manifest, files = make_generation()
    sync_mod.sync(FakeS3(publish(manifest, files)), BUCKET, tmp_path)
    after = snapshot(tmp_path)
    changed = {k for k in before if before[k] != after.get(k)}
    assert changed == {"current.json"}
    assert local_pointer(tmp_path) == index.pointer_for(manifest)
    index.verify_files(tmp_path / "generations" / old["build_id"], old)


def test_missing_pointer_tolerated_only_for_initial_migration(tmp_path):
    s3 = FakeS3({})
    with pytest.raises(sync_mod.MissingPointer):
        sync_mod.sync(s3, BUCKET, tmp_path)
    result = sync_mod.sync(s3, BUCKET, tmp_path, allow_missing_pointer=True)
    assert result["status"] == "no-remote-pointer"
    assert local_pointer(tmp_path) is None
    old = seed_legacy(tmp_path)
    with pytest.raises(sync_mod.MissingPointer):
        sync_mod.sync(s3, BUCKET, tmp_path, allow_missing_pointer=True)
    assert local_pointer(tmp_path) == index.pointer_for(old)


def test_access_denied_never_becomes_success(tmp_path):
    class Denied(FakeS3):
        def get_object(self, *, Bucket, Key):
            raise ClientError("AccessDenied")

    with pytest.raises(sync_mod.SyncError, match="Cannot read") as info:
        sync_mod.sync(Denied({}), BUCKET, tmp_path, allow_missing_pointer=True)
    assert not isinstance(info.value, sync_mod.MissingPointer)
    assert local_pointer(tmp_path) is None


def test_lock_contention_fails_without_activation(tmp_path):
    manifest, files = make_generation()
    s3 = FakeS3(publish(manifest, files))
    (tmp_path / "generations").mkdir()
    fd = os.open(tmp_path / sync_mod.LOCK_NAME, os.O_RDWR | os.O_CREAT)
    fcntl.flock(fd, fcntl.LOCK_EX)
    try:
        with pytest.raises(sync_mod.LockHeld):
            sync_mod.sync(s3, BUCKET, tmp_path, lock_timeout=0.2)
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)
    assert s3.requests == []
    assert local_pointer(tmp_path) is None
    assert sync_mod.sync(s3, BUCKET, tmp_path)["status"] == "installed"


def test_overlapping_writers_serialize_activation(tmp_path):
    manifest, files = make_generation()
    s3 = FakeS3(publish(manifest, files))
    outcomes = {}

    def racer(store, key, filename):
        if "started" in outcomes:
            return
        outcomes["started"] = True
        try:
            outcomes["second"] = sync_mod.sync(
                FakeS3(store.objects), BUCKET, tmp_path, lock_timeout=0.3
            )
        except sync_mod.LockHeld as error:
            outcomes["second"] = error
        outcomes["pointer_during_first"] = local_pointer(tmp_path)

    s3.on_download = racer
    result = sync_mod.sync(s3, BUCKET, tmp_path)
    assert result["status"] == "installed"
    assert isinstance(outcomes["second"], sync_mod.LockHeld)
    assert outcomes["pointer_during_first"] is None
    assert local_pointer(tmp_path) == index.pointer_for(manifest)

    waiter = {}

    def wait_for_lock():
        waiter["result"] = sync_mod.sync(FakeS3(s3.objects), BUCKET, tmp_path, lock_timeout=5)

    with sync_mod.destination_lock(tmp_path, 1):
        thread = threading.Thread(target=wait_for_lock)
        thread.start()
        thread.join(0.3)
        assert thread.is_alive()
    thread.join(5)
    assert waiter["result"]["status"] == "current"


@pytest.mark.parametrize("entry", ["generations", "current.json", sync_mod.LOCK_NAME])
def test_local_symlink_is_rejected_without_writing_outside_root(tmp_path, entry):
    root, outside = tmp_path / "root", tmp_path / "outside"
    root.mkdir()
    outside.mkdir()
    sentinel = outside / "sentinel"
    sentinel.write_bytes(b"unchanged")
    target = outside if entry == "generations" else sentinel
    (root / entry).symlink_to(target)
    manifest, files = make_generation()
    s3 = FakeS3(publish(manifest, files))
    with pytest.raises((OSError, ValueError)):
        sync_mod.sync(s3, BUCKET, root)
    assert sentinel.read_bytes() == b"unchanged"
    assert sorted(p.name for p in outside.iterdir()) == ["sentinel"]
    assert s3.requests == []


def test_symlink_destination_is_rejected(tmp_path):
    outside = tmp_path / "outside"
    outside.mkdir()
    root = tmp_path / "root"
    root.symlink_to(outside, target_is_directory=True)
    with pytest.raises(ValueError, match="symlink"):
        sync_mod.sync(FakeS3({}), BUCKET, root)
    assert list(outside.iterdir()) == []


def test_dangling_local_pointer_is_not_initial_migration(tmp_path):
    (tmp_path / "current.json").symlink_to(tmp_path / "missing")
    with pytest.raises(ValueError, match="symlink"):
        sync_mod.sync(FakeS3({}), BUCKET, tmp_path, allow_missing_pointer=True)


@pytest.mark.parametrize("timeout", [-1, float("inf"), float("nan")])
def test_invalid_lock_timeout_is_rejected(tmp_path, timeout):
    with pytest.raises(ValueError, match="finite and nonnegative"):
        sync_mod.sync(FakeS3({}), BUCKET, tmp_path, lock_timeout=timeout)


@pytest.mark.parametrize("declared_length", [0, sync_mod.MAX_POINTER_BYTES + 1])
def test_bounded_json_stream_is_closed_on_oversize(declared_length):
    stream = io.BytesIO(b" " * (sync_mod.MAX_POINTER_BYTES + 1))

    class Store:
        def get_object(self, **kwargs):
            return {"Body": stream, "ContentLength": declared_length}

    with pytest.raises(ValueError, match="exceeds"):
        sync_mod.read_json_object(
            Store(), BUCKET, "docs_db/current.json", sync_mod.MAX_POINTER_BYTES
        )
    assert stream.closed


def test_real_generation_round_trip_to_reader(tmp_path):
    from test_docs_index_corpus import corpus, vector_writer
    from test_docs_index_publication import Store

    source, destination = tmp_path / "source", tmp_path / "destination"
    path, manifest = index.build_generation(corpus(), source, vector_writer)
    store = Store()
    index.publish_generation(store, BUCKET, path, manifest)
    result = sync_mod.sync(FakeS3(store.objects), BUCKET, destination)
    conn, table, loaded = index.GenerationReader(destination).get()
    try:
        assert result["build_id"] == loaded["build_id"] == manifest["build_id"]
        sql_urls = {row[0] for row in conn.execute("SELECT url FROM pages")}
        vector_urls = {row["url"] for row in table.search().select(["url"]).to_list()}
        assert sql_urls == vector_urls == set(manifest["included_urls"])
    finally:
        conn.close()
