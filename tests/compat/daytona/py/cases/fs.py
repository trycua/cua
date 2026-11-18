"""sandbox.fs: transfers (single, bulk, streamed, signed URL), metadata,
errors (file codes), search and replace. `mock` cases run over the in-memory
guest; `space` cases need a real shell.

Derived from daytona 0.225.0 `_sync/filesystem.py` and `common/filesystem.py`."""

import hashlib
import os
import tempfile
import urllib.request

from harness import case, err_obs

PAYLOAD = bytes(range(256)) * 1031  # 264 KB, every byte value, not text


@case(
    "fs.roundtrip_binary_and_empty",
    needs="mock",
    ref="_sync/filesystem.py upload_file/download_file",
)
async def roundtrip(c):
    sb = await c.main()
    await c.w(sb.fs.upload_file(PAYLOAD, "/tmp/fsrt/bin.dat"))
    got = await c.w(sb.fs.download_file("/tmp/fsrt/bin.dat"))
    assert got == PAYLOAD
    await c.w(sb.fs.upload_file(b"", "/tmp/fsrt/empty"))
    assert await c.w(sb.fs.download_file("/tmp/fsrt/empty")) == b""
    text = "héllo wörld ✓".encode()
    await c.w(sb.fs.upload_file(text, "/tmp/fsrt/uni.txt"))
    assert await c.w(sb.fs.download_file("/tmp/fsrt/uni.txt")) == text
    return {
        "binary": len(got),
        "sha": hashlib.sha256(got).hexdigest()[:16],
        "empty": 0,
        "unicode": len(text),
    }


@case(
    "fs.unicode_and_space_names", needs="mock", ref="_sync/filesystem.py (paths in query/multipart)"
)
async def unicode_names(c):
    sb = await c.main()
    names = ["naïve café.txt", "with space.txt", "plus+sign&amp=eq.txt", "日本語.txt"]
    for n in names:
        await c.w(sb.fs.upload_file(n.encode(), f"/tmp/fsuni/{n}"))
    listed = sorted(f.name for f in await c.w(sb.fs.list_files("/tmp/fsuni")))
    assert listed == sorted(names), listed
    for n in names:
        assert await c.w(sb.fs.download_file(f"/tmp/fsuni/{n}")) == n.encode()
    return {"names": len(listed)}


@case(
    "fs.upload_creates_parents_and_overwrites", needs="mock", ref="_sync/filesystem.py upload_file"
)
async def upload_parents(c):
    sb = await c.main()
    await c.w(sb.fs.upload_file(b"one", "/tmp/fsp/a/b/c/f.txt"))
    await c.w(sb.fs.upload_file(b"two", "/tmp/fsp/a/b/c/f.txt"))
    assert await c.w(sb.fs.download_file("/tmp/fsp/a/b/c/f.txt")) == b"two"
    return {"overwritten": True}


@case(
    "fs.list_info_folder_move_delete",
    needs="mock",
    ref="_sync/filesystem.py list_files/get_file_info/create_folder/move_files/delete_file",
)
async def list_info(c):
    sb = await c.main()
    await c.w(sb.fs.create_folder("/tmp/fsl", "755"))
    await c.w(sb.fs.upload_file(b"payload", "/tmp/fsl/a.txt"))
    info = await c.w(sb.fs.get_file_info("/tmp/fsl/a.txt"))
    assert (info.name, info.size, info.is_dir) == ("a.txt", 7, False), info
    d = await c.w(sb.fs.get_file_info("/tmp/fsl"))
    assert d.is_dir
    names = sorted(f.name for f in await c.w(sb.fs.list_files("/tmp/fsl")))
    assert names == ["a.txt"], names
    await c.w(sb.fs.move_files("/tmp/fsl/a.txt", "/tmp/fsl/b.txt"))
    assert await c.w(sb.fs.download_file("/tmp/fsl/b.txt")) == b"payload"
    e = await c.raises(lambda: sb.fs.get_file_info("/tmp/fsl/a.txt"))
    assert e and e["code"] == "FILE_NOT_FOUND", e
    await c.w(sb.fs.delete_file("/tmp/fsl/b.txt"))
    e2 = await c.raises(lambda: sb.fs.download_file("/tmp/fsl/b.txt"))
    assert e2 and e2["code"] == "FILE_NOT_FOUND", e2
    return {"size": 7, "movedFrom": err_obs(e), "afterDelete": err_obs(e2)}


@case(
    "fs.errors_carry_file_codes",
    needs="mock",
    ref="common/errors.py CODE_TO_ERROR (FILE_NOT_FOUND)",
)
async def file_errors(c):
    sb = await c.main()
    out = {}
    for name, fn in {
        "download": lambda: sb.fs.download_file("/tmp/does-not-exist"),
        "info": lambda: sb.fs.get_file_info("/tmp/does-not-exist"),
        "delete": lambda: sb.fs.delete_file("/tmp/does-not-exist"),
        "move": lambda: sb.fs.move_files("/tmp/does-not-exist", "/tmp/x"),
    }.items():
        e = await c.raises(fn)
        assert e and e["type"] == "DaytonaFileNotFoundError" and e["status_code"] == 404, (name, e)
        assert e["code"] == "FILE_NOT_FOUND" and e["source"] == "DAYTONA_DAEMON", (name, e)
        out[name] = err_obs(e)
    return out


@case(
    "fs.bulk_upload_download", needs="mock", ref="_sync/filesystem.py upload_files/download_files"
)
async def bulk(c):
    sb = await c.main()
    sdk = c.sdk
    files = {f"/tmp/fsb/d{i % 3}/f{i}.bin": bytes([i]) * (i * 100 + 1) for i in range(30)}
    await c.w(
        sb.fs.upload_files([sdk.FileUpload(source=v, destination=k) for k, v in files.items()])
    )
    reqs = [sdk.FileDownloadRequest(source=k) for k in files] + [
        sdk.FileDownloadRequest(source="/tmp/fsb/missing")
    ]
    res = await c.w(sb.fs.download_files(reqs))
    assert len(res) == 31
    for r in res[:30]:
        assert r.error is None and r.result == files[r.source], r.source
    miss = res[30]
    assert miss.error and miss.result is None, miss
    # one with a local destination
    with tempfile.TemporaryDirectory() as d:
        dest = os.path.join(d, "sub", "out.bin")
        r = await c.w(
            sb.fs.download_files(
                [sdk.FileDownloadRequest(source="/tmp/fsb/d0/f0.bin", destination=dest)]
            )
        )
        assert r[0].result == dest and open(dest, "rb").read() == files["/tmp/fsb/d0/f0.bin"], r
    return {
        "ok": 30,
        "missingHasError": True,
        "missingStructured": miss.error_details is not None and miss.error_details.code,
    }


@case(
    "fs.download_to_local_path_and_stream",
    needs="mock",
    ref="_sync/filesystem.py download_file(remote, local)/download_file_stream",
)
async def local_and_stream(c):
    sb = await c.main()
    blob = os.urandom(3 * 1024 * 1024 + 17)
    await c.w(sb.fs.upload_file(blob, "/tmp/fss/blob.bin"))
    with tempfile.TemporaryDirectory() as d:
        dest = os.path.join(d, "blob.bin")
        await c.w(sb.fs.download_file("/tmp/fss/blob.bin", dest))
        assert open(dest, "rb").read() == blob
    progress = []
    chunks = await _chunks(
        sb.fs.download_file_stream("/tmp/fss/blob.bin", on_progress=progress.append)
    )
    assert b"".join(chunks) == blob and len(chunks) >= 2, len(chunks)
    assert progress and progress[-1].bytes_received == len(blob), progress[-1:]
    e = await c.raises(lambda: _consume(sb.fs.download_file_stream("/tmp/fss/none")))
    assert e and e["code"] == "FILE_NOT_FOUND", e
    return {
        "bytes": len(blob),
        "multipleChunks": True,
        "progressTotal": True,
        "missing": err_obs(e),
    }


async def _chunks(stream):
    """All chunks of a download stream (a generator in sync mode; a coroutine
    returning an async generator in async mode)."""
    if hasattr(stream, "__await__"):
        stream = await stream
    out = []
    if hasattr(stream, "__aiter__"):
        async for ch in stream:
            out.append(ch)
    else:
        for ch in stream:
            out.append(ch)
    return out


async def _consume(stream):
    await _chunks(stream)


@case("fs.upload_stream", needs="mock", ref="_sync/filesystem.py upload_file_stream (chunked body)")
async def upload_stream(c):
    import io

    sb = await c.main()
    blob = os.urandom(2 * 1024 * 1024 + 5)
    sent = []
    await c.w(sb.fs.upload_file_stream(io.BytesIO(blob), "/tmp/fsu/s.bin", on_progress=sent.append))
    assert await c.w(sb.fs.download_file("/tmp/fsu/s.bin")) == blob
    assert sent and sent[-1].bytes_sent == len(blob), sent[-1:]
    return {"bytes": len(blob), "progress": True}


@case(
    "fs.signed_urls",
    needs="mock",
    ref="_sync/sandbox.py download_url/upload_url; _utils/file_url_signing.py",
)
async def signed_urls(c):
    sb = await c.main()
    await c.w(sb.fs.upload_file(b"signed-payload", "/tmp/fsg/s.txt"))
    url = await c.w(sb.download_url("/tmp/fsg/s.txt", ttl_seconds=60))
    with urllib.request.urlopen(url, timeout=15) as r:
        assert r.read() == b"signed-payload"
    # a tampered signature is refused
    bad = url[:-3] + ("aaa" if not url.endswith("aaa") else "bbb")
    try:
        urllib.request.urlopen(bad, timeout=15)
        raise AssertionError("tampered URL accepted")
    except urllib.error.HTTPError as e:
        assert e.code == 403, e.code
    up = await c.w(sb.upload_url("/tmp/fsg/up.txt", ttl_seconds=60))
    req = urllib.request.Request(up, data=b"via-signed-upload", method="POST")
    urllib.request.urlopen(req, timeout=15).read()
    assert await c.w(sb.fs.download_file("/tmp/fsg/up.txt")) == b"via-signed-upload"
    # rotating the key invalidates old URLs
    await c.w(sb.rotate_signing_key())
    try:
        urllib.request.urlopen(url, timeout=15)
        raise AssertionError("an old URL survived a key rotation")
    except urllib.error.HTTPError as e:
        assert e.code == 403, e.code
    return {"download": True, "tampered": 403, "upload": True, "rotated": 403}


@case("fs.concurrent_transfers", needs="mock", ref="_sync/filesystem.py (shared connection pool)")
async def concurrent(c):
    import asyncio
    import concurrent.futures

    sb = await c.main()
    blobs = {f"/tmp/fsc/{i}.bin": os.urandom(50_000 + i) for i in range(16)}

    if c.is_async:
        await asyncio.gather(*(sb.fs.upload_file(v, k) for k, v in blobs.items()))
        got = await asyncio.gather(*(sb.fs.download_file(k) for k in blobs))
    else:
        with concurrent.futures.ThreadPoolExecutor(8) as ex:
            list(ex.map(lambda kv: sb.fs.upload_file(kv[1], kv[0]), blobs.items()))
            got = list(ex.map(lambda k: sb.fs.download_file(k), blobs))
    assert all(g == v for g, v in zip(got, blobs.values()))
    return {"files": len(got)}


@case(
    "fs.large_file_integrity",
    needs="space",
    ref="_sync/filesystem.py upload_file/download_file_stream",
)
async def large_file(c):
    sb = await c.main()
    size = 96 * 1024 * 1024
    with tempfile.NamedTemporaryFile(delete=False) as f:
        h = hashlib.sha256()
        for _ in range(size // (1024 * 1024)):
            blk = os.urandom(1024 * 1024)
            h.update(blk)
            f.write(blk)
        src = f.name
    try:
        await c.w(sb.fs.upload_file(src, "/tmp/fsbig/big.bin"))
        r = await c.w(sb.process.exec("sha256sum /tmp/fsbig/big.bin"))
        assert r.result.split()[0] == h.hexdigest(), r.result
        got = hashlib.sha256()
        for ch in await _chunks(sb.fs.download_file_stream("/tmp/fsbig/big.bin")):
            got.update(ch)
        assert got.hexdigest() == h.hexdigest()
    finally:
        os.unlink(src)
    return {"mib": size // (1024 * 1024), "sha256Matches": True}


@case(
    "fs.permissions_and_owner",
    needs="space",
    ref="_sync/filesystem.py set_file_permissions/get_file_info",
)
async def permissions(c):
    sb = await c.main()
    await c.w(sb.fs.upload_file(b"x", "/tmp/fsperm/f.txt"))
    await c.w(sb.fs.create_folder("/tmp/fsperm/dir", "700"))
    d = await c.w(sb.fs.get_file_info("/tmp/fsperm/dir"))
    assert d.permissions.endswith("700"), d.permissions
    await c.w(sb.fs.set_file_permissions("/tmp/fsperm/f.txt", mode="640"))
    f = await c.w(sb.fs.get_file_info("/tmp/fsperm/f.txt"))
    assert f.permissions.endswith("640"), f.permissions
    assert f.owner == "daytona", f.owner
    return {"dir": "700", "file": "640", "owner": f.owner}


@case(
    "fs.find_search_replace",
    needs="space",
    ref="_sync/filesystem.py find_files/search_files/replace_in_files",
)
async def find_search_replace(c):
    sb = await c.main()
    sdk = c.sdk
    await c.w(
        sb.fs.upload_files(
            [
                sdk.FileUpload(source=b"alpha\nneedle one\nomega\n", destination="/tmp/fsfs/a.txt"),
                sdk.FileUpload(
                    source=b"nothing here\nneedle two\n", destination="/tmp/fsfs/sub/b.txt"
                ),
                sdk.FileUpload(source=b"x", destination="/tmp/fsfs/sub/c.log"),
            ]
        )
    )
    found = await c.w(sb.fs.find_files("/tmp/fsfs", "needle"))
    got = sorted((m.file.rsplit("/", 1)[-1], m.line, m.content.strip()) for m in found)
    assert got == [("a.txt", 2, "needle one"), ("b.txt", 2, "needle two")], got
    s = await c.w(sb.fs.search_files("/tmp/fsfs", "*.txt"))
    assert sorted(f.rsplit("/", 1)[-1] for f in s.files) == ["a.txt", "b.txt"], s.files
    rep = await c.w(
        sb.fs.replace_in_files(
            ["/tmp/fsfs/a.txt", "/tmp/fsfs/sub/b.txt", "/tmp/fsfs/nope.txt"], "needle", "thread"
        )
    )
    ok = {r.file.rsplit("/", 1)[-1]: r.success for r in rep}
    assert ok["a.txt"] and ok["b.txt"] and not ok["nope.txt"], ok
    assert b"thread one" in await c.w(sb.fs.download_file("/tmp/fsfs/a.txt"))
    return {"found": got, "search": ["a.txt", "b.txt"], "replaced": ok}
