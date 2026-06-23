"""
Package a DaaS KiCad submission JSON into a self-contained cb task folder.

Usage:
    python package_kicad_submission.py <submission.json> --output <output_dir>

The submission JSON must contain:
    - submission_id
    - circuit_pcb_file.s3Uri
    - netlist.s3Uri
    - circuit_prompt  (used as task description)
    - difficulty
"""
from __future__ import annotations

import argparse
import io
import json
import re
import zipfile
from pathlib import Path

import boto3

# Patterns to skip when extracting KiCad zips
_SKIP_PREFIXES = ("__MACOSX", "._", "_autosave-", "#auto_saved_files#", "~")
_SKIP_SUFFIXES = ("-backups", ".DS_Store")

_REMOTE_PROJECT_DIR = "/home/cua/kicad_project"

_MAIN_PY_TEMPLATE = '''\
"""Auto-generated cb task for KiCad submission {submission_id}."""
from __future__ import annotations

import asyncio
from pathlib import Path

import cua_bench as cb

_SUBMISSION_ID = "{submission_id}"
_REMOTE_PROJECT_DIR = "{remote_project_dir}"
_HARNESS_DIR = Path(__file__).parent


@cb.tasks_config
def tasks() -> list[cb.Task]:
    return [
        cb.Task(
            description={description!r},
            metadata={{"difficulty": {difficulty!r}, "submission_id": _SUBMISSION_ID}},
        )
    ]


@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession) -> None:
    await session.apps.kicad.install(with_shortcut=True)

    initial_dir = _HARNESS_DIR / "initial"
    for local_path in sorted(initial_dir.rglob("*")):
        if not local_path.is_file():
            continue
        rel = local_path.relative_to(initial_dir)
        remote_path = f"{{_REMOTE_PROJECT_DIR}}/{{rel.as_posix()}}"
        remote_dir = f"{{_REMOTE_PROJECT_DIR}}/{{rel.parent.as_posix()}}"
        await session.run_command(f"mkdir -p '{{remote_dir}}'", check=False)
        await session.write_bytes(remote_path, local_path.read_bytes())

    try:
        await session.apps.kicad.launch(project_path={project_path!r})
    except Exception:
        pass
    await asyncio.sleep(5)


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession) -> None:
    """Oracle solver: upload reference.net directly to the expected output path."""
    await session.run_command(f"mkdir -p '{{_REMOTE_PROJECT_DIR}}'", check=False)
    await session.write_bytes(
        f"{{_REMOTE_PROJECT_DIR}}/output.net",
        (_HARNESS_DIR / "reference.net").read_bytes(),
    )


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> float:
    from cua_bench.netlist_compare import (
        compare_kicad_netlists,
        load_reference_netlist,
    )

    reference = load_reference_netlist(_HARNESS_DIR / "reference.net", _HARNESS_DIR)

    # Try common netlist output locations first
    candidate_paths = [
        f"{{_REMOTE_PROJECT_DIR}}/{{_SUBMISSION_ID}}.net",
        f"{{_REMOTE_PROJECT_DIR}}/output.net",
    ]
    for path in candidate_paths:
        result = await session.run_command(f"cat '{{path}}'", check=False)
        candidate = result.get("stdout", "")
        if candidate.strip():
            return compare_kicad_netlists(candidate, reference)

    # Search for any .net file in the project dir
    result = await session.run_command(
        f"find {{_REMOTE_PROJECT_DIR}} -name '*.net' | head -1", check=False
    )
    net_path = result.get("stdout", "").strip()
    if net_path:
        result = await session.run_command(f"cat '{{net_path}}'", check=False)
        candidate = result.get("stdout", "")
        if candidate.strip():
            return compare_kicad_netlists(candidate, reference)

    # Fall back: agent edited schematic without exporting — use kicad-cli to generate netlist
    result = await session.run_command(
        f"find {{_REMOTE_PROJECT_DIR}} -name '*.kicad_sch' | head -1", check=False
    )
    sch_path = result.get("stdout", "").strip()
    if sch_path:
        out_net = "/tmp/kicad_eval_output.net"
        await session.run_command(
            f"kicad-cli sch export netlist '{{sch_path}}' -o '{{out_net}}'",
            check=False,
        )
        result = await session.run_command(f"cat '{{out_net}}'", check=False)
        candidate = result.get("stdout", "")
        if candidate.strip():
            return compare_kicad_netlists(candidate, reference)

    return 0.0
'''


def _should_skip(name: str) -> bool:
    parts = name.replace("\\", "/").split("/")
    for part in parts:
        if any(part.startswith(p) for p in _SKIP_PREFIXES):
            return True
        if any(part.endswith(s) for s in _SKIP_SUFFIXES):
            return True
    return False


def _download_zip(s3_uri: str) -> zipfile.ZipFile:
    s3 = boto3.client("s3", region_name="us-west-2")
    bucket, key = s3_uri.replace("s3://", "").split("/", 1)
    obj = s3.get_object(Bucket=bucket, Key=key)
    return zipfile.ZipFile(io.BytesIO(obj["Body"].read()))


def _first_net_file(zf: zipfile.ZipFile) -> bytes:
    for name in zf.namelist():
        if _should_skip(name):
            continue
        if name.endswith(".net") and not name.endswith("/"):
            return zf.read(name)
    raise ValueError("No .net file found in netlist zip")


def _extract_kicad_zip(zf: zipfile.ZipFile, dest: Path) -> None:
    dest.mkdir(parents=True, exist_ok=True)
    for name in zf.namelist():
        if _should_skip(name):
            continue
        if name.endswith("/"):
            continue
        out = dest / name
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(zf.read(name))


def _slugify(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", "_", text.lower()).strip("_")[:40]


def package(submission: dict, output_dir: Path) -> None:
    sub_id = submission["submission_id"]
    initial_circuit = submission.get("initial_circuit")
    pcb_uri = initial_circuit["s3Uri"] if initial_circuit else None
    netlist_uri = submission["netlist"]["s3Uri"]
    description = submission.get("circuit_prompt", "").strip()
    difficulty = submission.get("difficulty", "unknown")

    output_dir.mkdir(parents=True, exist_ok=True)

    # Download and extract netlist → reference.net
    print(f"  Downloading netlist: {netlist_uri}")
    netlist_zip = _download_zip(netlist_uri)
    reference_net = _first_net_file(netlist_zip)
    (output_dir / "reference.net").write_bytes(reference_net)

    # Download and extract PCB files → initial/ (empty for start-from-scratch tasks)
    if pcb_uri:
        print(f"  Downloading PCB: {pcb_uri}")
        pcb_zip = _download_zip(pcb_uri)
        _extract_kicad_zip(pcb_zip, output_dir / "initial")
    else:
        print("  No initial circuit (start from scratch)")
        (output_dir / "initial").mkdir(parents=True, exist_ok=True)

    # Derive remote .kicad_pro path from the zip filename (stem = folder and project name)
    if initial_circuit:
        stem = Path(initial_circuit["filename"]).stem  # e.g. "kicad_555timer_circuit"
        project_path = f"{_REMOTE_PROJECT_DIR}/{stem}/{stem}.kicad_pro"
    else:
        project_path = None

    # Generate main.py
    main_py = _MAIN_PY_TEMPLATE.format(
        submission_id=sub_id,
        remote_project_dir=_REMOTE_PROJECT_DIR,
        description=description,
        difficulty=difficulty,
        project_path=project_path,
    )
    (output_dir / "main.py").write_text(main_py, encoding="utf-8")

    # Copy submission.json
    (output_dir / "submission.json").write_text(
        json.dumps(submission, indent=2), encoding="utf-8"
    )

    print(f"  → {output_dir}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("submission_json", help="Path to submission JSON file")
    parser.add_argument("--output", required=True, help="Output directory")
    args = parser.parse_args()

    submission = json.loads(Path(args.submission_json).read_text())
    package(submission, Path(args.output))


if __name__ == "__main__":
    main()
