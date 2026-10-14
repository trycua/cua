"""Local generation cutover and fake object-store publication failure tests."""

import json
import sqlite3

import pytest

from test_docs_index_corpus import corpus, index, vector_writer


class Store:
    def __init__(self, fail_after=None):
        self.objects = {"docs_db/current.json": b"previous"}
        self.fail_after = fail_after
        self.uploads = []

    def upload_file(self, path, bucket, key):
        if self.fail_after == len(self.uploads):
            raise OSError("Synthetic interrupted upload")
        from pathlib import Path

        self.objects[key] = Path(path).read_bytes()
        self.uploads.append(key)

    def put_object(self, *, Bucket, Key, Body, ContentType):
        self.objects[Key] = Body
        self.uploads.append(Key)


def test_incomplete_publication_retains_previous_pointer(tmp_path):
    path, manifest = index.build_generation(corpus(), tmp_path, vector_writer)
    store = Store(fail_after=1)
    with pytest.raises(OSError, match="interrupted upload"):
        index.publish_generation(store, "test-bucket", path, manifest)
    assert store.objects["docs_db/current.json"] == b"previous"


def test_publication_marker_is_last_and_generation_is_complete(tmp_path):
    path, manifest = index.build_generation(corpus(), tmp_path, vector_writer)
    store = Store()
    index.publish_generation(store, "test-bucket", path, manifest)
    assert store.uploads[-1] == "docs_db/current.json"
    assert json.loads(store.objects[store.uploads[-1]]) == index.pointer_for(manifest)
    prefix = f"docs_db/generations/{manifest['build_id']}/"
    assert {prefix + name for name in manifest["files"]} <= set(store.objects)


def test_reader_refreshes_both_handles_without_restart(tmp_path):
    _, first = index.build_generation(corpus(), tmp_path, vector_writer)
    index.activate(tmp_path, first)
    reader = index.GenerationReader(tmp_path)
    old_conn, old_table, _ = reader.get()
    _, second = index.build_generation(corpus(), tmp_path, vector_writer)
    index.activate(tmp_path, second)
    conn, table, manifest = reader.get()
    assert manifest["build_id"] == second["build_id"]
    assert conn is not old_conn and table is not old_table
    assert (
        old_conn.execute("SELECT build_id FROM index_metadata").fetchone()[0] == first["build_id"]
    )
    assert conn.execute("SELECT build_id FROM index_metadata").fetchone()[0] == second["build_id"]
    assert {r["build_id"] for r in table.search().select(["build_id"]).to_list()} == {
        second["build_id"]
    }


def test_incomplete_consumer_copy_keeps_last_good_until_complete(tmp_path):
    _, first = index.build_generation(corpus(), tmp_path, vector_writer)
    index.activate(tmp_path, first)
    reader = index.GenerationReader(tmp_path)
    previous = reader.get()
    path, second = index.build_generation(corpus(), tmp_path, vector_writer)
    database = path / "docs.sqlite"
    database.rename(path / "pending.sqlite")
    index.write_json(tmp_path / "current.json", index.pointer_for(second))
    assert reader.get() is previous
    assert reader.error is not None
    (path / "pending.sqlite").rename(database)
    assert reader.get()[2]["build_id"] == second["build_id"]
    assert reader.error is None


def test_mismatched_pointer_and_corrupt_file_keep_last_good(tmp_path):
    _, first = index.build_generation(corpus(), tmp_path, vector_writer)
    index.activate(tmp_path, first)
    reader = index.GenerationReader(tmp_path)
    previous = reader.get()
    path, second = index.build_generation(corpus(), tmp_path, vector_writer)
    index.write_json(
        tmp_path / "current.json", {**index.pointer_for(second), "manifest_sha256": "wrong"}
    )
    assert reader.get() is previous
    assert "identity mismatch" in reader.error
    with sqlite3.connect(path / "docs.sqlite") as conn:
        conn.execute("UPDATE pages SET build_id = 'wrong'")
    index.write_json(tmp_path / "current.json", index.pointer_for(second))
    assert reader.get() is previous
    assert "Incomplete docs generation" in reader.error


def test_legacy_indexes_are_not_claimed_as_verified(tmp_path):
    (tmp_path / "docs.sqlite").touch()
    with pytest.raises(RuntimeError, match="No verified docs generation"):
        index.GenerationReader(tmp_path).get()


def test_copy_then_refresh_and_failed_volume_reload(tmp_path):
    source, destination = tmp_path / "source", tmp_path / "reader"
    _, first = index.build_generation(corpus(), source, vector_writer)
    index.activate(source, first)
    reader = index.GenerationReader(
        destination, refresh=lambda: index.copy_generation(source, destination)
    )
    previous = reader.get()
    path, second = index.build_generation(corpus(), source, vector_writer)
    index.activate(source, second)
    database = path / "docs.sqlite"
    database.rename(path / "pending.sqlite")
    assert reader.get() is previous
    assert json.loads((destination / "current.json").read_text()) == index.pointer_for(first)
    (path / "pending.sqlite").rename(database)
    assert reader.get()[2]["build_id"] == second["build_id"]

    def failed_reload():
        raise OSError("Synthetic volume reload failure")

    reader.refresh = failed_reload
    assert reader.get()[2]["build_id"] == second["build_id"]
    assert "volume reload failure" in reader.error


def test_changed_manifest_cannot_be_published(tmp_path):
    path, manifest = index.build_generation(corpus(), tmp_path, vector_writer)
    index.write_json(path / "manifest.json", {**manifest, "corpus_id": "changed"})
    store = Store()
    with pytest.raises(ValueError, match="manifest mismatch"):
        index.publish_generation(store, "test-bucket", path, manifest)
    assert store.uploads == []


def test_reader_rejects_a_missing_sql_page_even_with_updated_hashes(tmp_path):
    path, manifest = index.build_generation(corpus(), tmp_path, vector_writer)
    with sqlite3.connect(path / "docs.sqlite") as conn:
        conn.execute("DELETE FROM pages WHERE id = 1")
    manifest["files"]["docs.sqlite"] = index.file_hash(path / "docs.sqlite")
    index.write_json(path / "manifest.json", manifest)
    index.activate(tmp_path, manifest)
    with pytest.raises(RuntimeError, match="No verified docs generation"):
        index.GenerationReader(tmp_path).get()


def test_scheduled_publication_does_not_mutate_the_other_index():
    """Validate schedule wiring without importing or calling the remote service."""
    import ast
    from pathlib import Path

    source = Path(__file__).resolve().parents[1] / "modal_app.py"
    module = ast.parse(source.read_text())
    for name, excluded in (
        ("scheduled_crawl", "publish_code"),
        ("scheduled_code_index", "publish_docs"),
    ):
        function = next(node for node in module.body if getattr(node, "name", None) == name)
        calls = [
            node
            for node in ast.walk(function)
            if isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "sync_to_s3"
        ]
        assert len(calls) == 1
        assert any(
            keyword.arg == excluded
            and isinstance(keyword.value, ast.Constant)
            and keyword.value.value is False
            for keyword in calls[0].keywords
        )
