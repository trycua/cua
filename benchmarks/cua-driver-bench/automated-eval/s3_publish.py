"""Publish generated benchmark reports as a static S3 bundle."""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import urllib.error
import urllib.parse
import urllib.request
from datetime import UTC, datetime
from pathlib import Path, PurePosixPath

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


def _report_url(bucket: str, region: str, object_key: str) -> str:
    encoded_key = "/".join(
        urllib.parse.quote(part, safe="") for part in PurePosixPath(object_key).parts
    )
    return f"https://{bucket}.s3.{region}.amazonaws.com/{encoded_key}"


def _verify_public_url(url: str, s3_root: str) -> None:
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "cua-driver-bench-report-publisher"},
        method="GET",
    )
    try:
        with urllib.request.urlopen(request, timeout=30.0) as response:
            response.read(1)
    except urllib.error.HTTPError as error:
        if error.code == 403:
            raise RuntimeError(
                "Report uploaded successfully to:\n\n"
                f"{s3_root}\n\n"
                "but the HTTP object is not publicly readable.\n\n"
                "A single presigned index.html URL is not sufficient because the report "
                "contains linked HTML and image assets. Configure public read/static hosting "
                "for this report prefix or add a private static-distribution layer such as "
                "CloudFront later."
            ) from error
        raise RuntimeError(f"published report URL returned HTTP {error.code}: {url}") from error
    except urllib.error.URLError as error:
        raise RuntimeError(f"could not read published report URL {url}: {error.reason}") from error


def publish_report(
    run_dir: Path,
    *,
    bucket: str,
    prefix: str,
    profile: str | None,
    region: str | None,
) -> str:
    """Upload one completed report bundle and return its public index URL."""
    report_dir, markdown_path, json_path = _validate_report_bundle(run_dir)
    bucket_name = _validate_bucket(bucket)
    prefix_name = _normalize_prefix(prefix)
    aws = shutil.which("aws")
    if aws is None:
        raise RuntimeError("AWS CLI is unavailable; install aws and ensure it is on PATH")
    resolved_region = _resolve_region(aws, region, profile)
    object_root = f"{prefix_name}/{_run_id()}"
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

    url = _report_url(bucket_name, resolved_region, f"{object_root}/index.html")
    print("[s3] verifying public URL")
    _verify_public_url(url, s3_root)
    print("[s3] report available")
    return url
