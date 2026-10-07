"""Publish generated benchmark reports as a static S3 bundle."""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import urllib.parse
from datetime import UTC, datetime
from pathlib import Path

_GIT_SHA = re.compile(r"^[0-9a-fA-F]{7,40}$")


def _validate_bucket(bucket: str) -> str:
    value = bucket.strip()
    if not value or "/" in value or any(character.isspace() for character in value):
        raise ValueError("AWS_S3_BUCKET must be a non-empty S3 bucket name")
    return value


def _normalize_prefix(prefix: str) -> str:
    parts = tuple(part for part in prefix.strip().split("/") if part)
    if not parts or any(part in {".", ".."} for part in parts):
        raise ValueError("AWS_S3_REPORT_PREFIX must contain a valid object-key prefix")
    return "/".join(parts)


def _normalize_report_base_url(base_url: str) -> str:
    value = base_url.strip().rstrip("/")
    parsed = urllib.parse.urlsplit(value)
    if parsed.scheme != "https" or not parsed.netloc or parsed.path not in {"", "/"}:
        raise ValueError("CDB_REPORT_BASE_URL must be an HTTPS origin without a path")
    if parsed.query or parsed.fragment or parsed.username or parsed.password:
        raise ValueError("CDB_REPORT_BASE_URL must be an HTTPS origin without credentials")
    return value


def _validate_report_bundle(run_dir: Path) -> tuple[Path, Path, Path]:
    root = run_dir.expanduser().resolve()
    report_dir = root / "report"
    index_path = report_dir / "index.html"
    markdown_path = root / "comparison.md"
    json_path = root / "comparison.json"
    required = (index_path, markdown_path, json_path)
    missing = [str(path) for path in required if not path.is_file()]
    if missing:
        raise ValueError("report bundle is incomplete; missing: " + ", ".join(missing))
    return report_dir, markdown_path, json_path


def _git_short_sha() -> str | None:
    repo_root = Path(__file__).resolve().parents[1]
    try:
        completed = subprocess.run(
            ["git", "rev-parse", "--short=7", "HEAD"],
            cwd=repo_root,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
            timeout=10.0,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    value = completed.stdout.strip()
    return value.casefold() if completed.returncode == 0 and _GIT_SHA.fullmatch(value) else None


def _run_id() -> str:
    stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    git_sha = _git_short_sha()
    return f"{stamp}-{git_sha}" if git_sha else stamp


def _aws_options(profile: str | None, region: str | None) -> list[str]:
    options: list[str] = []
    if profile:
        options.extend(("--profile", profile))
    if region:
        options.extend(("--region", region))
    return options


def _run_aws(
    aws: str,
    arguments: list[str],
    *,
    operation: str,
    destination: str,
    profile: str | None,
    region: str | None,
) -> None:
    completed = subprocess.run(
        [aws, *arguments, *_aws_options(profile, region)],
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    if completed.returncode == 0:
        return
    detail = (
        completed.stderr.strip() or completed.stdout.strip() or "AWS CLI returned no error text"
    )
    raise RuntimeError(f"{operation} failed for {destination}: {detail}")


def _resolve_region(aws: str, region: str | None, profile: str | None) -> str:
    configured = region or os.environ.get("AWS_REGION") or os.environ.get("AWS_DEFAULT_REGION")
    if configured and configured.strip():
        return configured.strip()
    completed = subprocess.run(
        [aws, "configure", "get", "region", *_aws_options(profile, None)],
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    discovered = completed.stdout.strip()
    if completed.returncode == 0 and discovered:
        return discovered
    raise ValueError(
        "AWS region is not configured; set AWS_REGION or configure the selected AWS profile"
    )


def _report_url(base_url: str, run_id: str) -> str:
    encoded_run_id = urllib.parse.quote(run_id, safe="")
    return f"{_normalize_report_base_url(base_url)}/{encoded_run_id}/index.html"


def publish_report(
    run_dir: Path,
    *,
    bucket: str,
    prefix: str,
    profile: str | None,
    region: str | None,
    report_base_url: str = "https://bench.trycua.com",
) -> tuple[str, str]:
    """Upload one completed report bundle and return its S3 URI and report URL."""
    report_dir, markdown_path, json_path = _validate_report_bundle(run_dir)
    bucket_name = _validate_bucket(bucket)
    prefix_name = _normalize_prefix(prefix)
    aws = shutil.which("aws")
    if aws is None:
        raise RuntimeError("AWS CLI is unavailable; install aws and ensure it is on PATH")
    resolved_region = _resolve_region(aws, region, profile)
    run_id = _run_id()
    object_root = f"{prefix_name}/{run_id}"
    s3_root = f"s3://{bucket_name}/{object_root}/"

    print(f"[s3] uploading report to {s3_root}")
    _run_aws(
        aws,
        [
            "s3",
            "sync",
            str(report_dir),
            s3_root,
            "--no-follow-symlinks",
            "--only-show-errors",
        ],
        operation="report upload",
        destination=s3_root,
        profile=profile,
        region=resolved_region,
    )
    for source in (markdown_path, json_path):
        destination = s3_root + source.name
        _run_aws(
            aws,
            ["s3", "cp", str(source), destination, "--only-show-errors"],
            operation=f"{source.name} upload",
            destination=destination,
            profile=profile,
            region=resolved_region,
        )
    print("[s3] uploaded report")

    index_key = f"{object_root}/index.html"
    print("[s3] verifying uploaded index.html")
    _run_aws(
        aws,
        ["s3api", "head-object", "--bucket", bucket_name, "--key", index_key],
        operation="report verification",
        destination=s3_root + "index.html",
        profile=profile,
        region=resolved_region,
    )
    print("[s3] report uploaded and verified")
    return s3_root, _report_url(report_base_url, run_id)
