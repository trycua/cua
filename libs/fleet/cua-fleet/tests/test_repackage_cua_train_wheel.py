from __future__ import annotations

import base64
import csv
import hashlib
from io import StringIO
from pathlib import Path
import sys
import zipfile

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from repackage_cua_train_wheel import WheelError, repack, verify  # noqa: E402


def _record(data: bytes) -> tuple[str, str]:
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
    return f"sha256={digest}", str(len(data))


def _csv(rows: list[list[str]]) -> bytes:
    output = StringIO()
    csv.writer(output, lineterminator="\n").writerows(rows)
    return output.getvalue().encode()


def _write_source_wheel(path: Path) -> dict[str, bytes]:
    dist_info = "cua_train-0.1.4.dist-info"
    entries = {
        "cua_train/__init__.py": b"TrainClient = object\n",
        "fleet_sdk/__init__.py": b"CyclopsClient = object\n",
        "fleet_sdk/libcyclops_sdk.so": b"native payload",
        f"{dist_info}/METADATA": b"Metadata-Version: 2.4\nName: cua-train\nVersion: 0.1.4\nDescription-Content-Type: text/markdown\n\n",
        f"{dist_info}/WHEEL": b"Wheel-Version: 1.0\nRoot-Is-Purelib: false\nTag: py3-none-manylinux_2_34_x86_64\n",
    }
    record_name = f"{dist_info}/RECORD"
    rows = [[name, *_record(data)] for name, data in sorted(entries.items())]
    rows.append([record_name, "", ""])
    entries[record_name] = _csv(rows)
    with zipfile.ZipFile(path, "w") as archive:
        for name, data in entries.items():
            archive.writestr(name, data)
    return entries


def test_repack_updates_distribution_metadata_and_pins_train(tmp_path: Path):
    source = tmp_path / "cua_train-0.1.4-py3-none-manylinux_2_34_x86_64.whl"
    source_entries = _write_source_wheel(source)

    destination = repack(
        source, "0.0.8", tmp_path / "dist", hashlib.sha256(source.read_bytes()).hexdigest()
    )

    assert destination.name == "cua_fleet-0.0.8-py3-none-manylinux_2_34_x86_64.whl"
    verify(destination, source)
    with zipfile.ZipFile(destination) as archive:
        assert archive.read("cua_train/__init__.py") == source_entries["cua_train/__init__.py"]
        assert (
            archive.read("fleet_sdk/libcyclops_sdk.so")
            == source_entries["fleet_sdk/libcyclops_sdk.so"]
        )
        metadata = archive.read("cua_fleet-0.0.8.dist-info/METADATA").decode()
        assert "Name: cua-fleet" in metadata
        assert "Version: 0.0.8" in metadata
        assert "Description-Content-Type: text/plain" in metadata
        assert "Requires-Dist: cua-train == 0.1.4" in metadata


def test_repack_rejects_unpinned_source(tmp_path: Path):
    source = tmp_path / "cua_train-0.1.4-py3-none-manylinux_2_34_x86_64.whl"
    _write_source_wheel(source)

    with pytest.raises(WheelError, match="SHA-256 mismatch"):
        repack(source, "0.0.8", tmp_path / "dist", "0" * 64)
