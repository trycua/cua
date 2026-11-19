#!/usr/bin/env python3
"""Exercise a Cua Perception release artifact through an installed Cua Driver.

The release gate runs this script on each target runner with the Driver that
the canonical installer resolves. It proves the user path end to end:

1. the extension is absent and ``perception parse`` refuses with ``not_installed``;
2. ``extension inspect`` verifies the artifact without mutating state;
3. ``extension install`` activates it and ``extension status --self-test`` is healthy;
4. ``perception parse`` runs the installed worker on a real PNG; and
5. ``extension remove`` restores the absent state.

A signed catalog must verify as production publisher evidence. The
developer-only ``--unsigned-archive`` mode exists for pull request dry runs of
the unsigned candidate and can never report publisher-verified trust.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any, Sequence

EXTENSION_ID = "cua-perception"
PRODUCTION_TRUST = "publisher-verified"
PRODUCTION_EVIDENCE = "production-publisher-verified"
PRODUCTION_PUBLISHER = "cua"
DEVELOPER_TRUST = "developer-unsigned-local"
DEVELOPER_EVIDENCE = "developer-local-not-publisher-evidence"
VISUAL_REGIONS_SCHEMA = "cua.visual_regions_v1"


class InstallCheckError(RuntimeError):
    """Raised when the installed Driver does not honor the release contract."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise InstallCheckError(message)


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


class Driver:
    def __init__(self, executable: Path, home: Path) -> None:
        # Resolve so a relative path never falls back to a Driver on PATH.
        self.executable = executable.resolve(strict=True)
        self.environment = dict(os.environ)
        self.environment.update({
            "CUA_DRIVER_RS_HOME": str(home),
            "CUA_DRIVER_RS_TELEMETRY_ENABLED": "false",
            "CUA_TELEMETRY_ENABLED": "false",
        })

    def run(self, arguments: Sequence[str], *, expect_success: bool = True) -> subprocess.CompletedProcess[str]:
        completed = subprocess.run(
            [str(self.executable), *arguments],
            capture_output=True,
            text=True,
            env=self.environment,
            timeout=900,
            check=False,
        )
        if expect_success and completed.returncode != 0:
            raise InstallCheckError(
                f"cua-driver {' '.join(arguments)} exited {completed.returncode}: "
                f"{completed.stderr.strip()[-2000:]}"
            )
        return completed

    def json(self, arguments: Sequence[str], *, expect_success: bool = True) -> dict[str, Any]:
        completed = self.run(arguments, expect_success=expect_success)
        try:
            value = json.loads(completed.stdout)
        except json.JSONDecodeError as error:
            raise InstallCheckError(
                f"cua-driver {' '.join(arguments)} did not print one JSON document: {error}"
            ) from error
        require(isinstance(value, dict), f"cua-driver {' '.join(arguments)} JSON is not an object")
        return value


def source_arguments(catalog: Path | None, archive: Path | None) -> list[str]:
    if catalog is not None:
        return ["--catalog", str(catalog)]
    assert archive is not None
    return ["--archive", str(archive), "--allow-unsigned-local"]


def check_absent(driver: Driver, image: Path, capture: Path) -> None:
    status = driver.json(["extension", "status", EXTENSION_ID, "--json"])
    require(status.get("id") == EXTENSION_ID, "status reported a different extension")
    require(status.get("installed") is False, "extension is already installed in the isolated home")
    refusal = driver.json(
        ["perception", "parse", "--image", str(image), "--capture", str(capture), "--json"],
        expect_success=False,
    )
    require(refusal.get("ok") is False, "parse without the extension did not fail closed")
    require(
        refusal.get("error", {}).get("code") == "not_installed",
        f"parse without the extension returned {refusal.get('error')!r}, not not_installed",
    )


def check_inspect(
    preview: dict[str, Any], *, signed: bool, version: str, target: str, source_sha: str,
    payload: dict[str, Any] | None,
) -> None:
    require(preview.get("id") == EXTENSION_ID, "inspect reported a different extension")
    require(preview.get("version") == version, f"inspect version {preview.get('version')!r} != {version}")
    require(preview.get("target") == target, f"inspect target {preview.get('target')!r} != {target}")
    require(preview.get("installed") is False, "inspect reported an installed extension")
    require(preview.get("mutation_performed") is False, "inspect mutated extension state")
    require(preview.get("ran") is False, "inspect ran the extension")
    require(
        preview.get("corresponding_source_revision") == source_sha,
        "inspect corresponding-source revision differs from the release commit",
    )
    require(bool(preview.get("models")), "inspect listed no models")
    require(bool(preview.get("license_notices")), "inspect listed no license notices")
    require(bool(preview.get("corresponding_source")), "inspect listed no corresponding source")
    if signed:
        assert payload is not None
        require(preview.get("trust") == PRODUCTION_TRUST, f"inspect trust is {preview.get('trust')!r}")
        require(
            preview.get("evidence_class") == PRODUCTION_EVIDENCE,
            f"inspect evidence class is {preview.get('evidence_class')!r}",
        )
        require(preview.get("publisher_signature_verified") is True, "publisher signature not verified")
        require(preview.get("publisher_id") == PRODUCTION_PUBLISHER, "inspect publisher is not cua")
        require(preview.get("publisher_key_id") == payload["key_id"], "inspect key differs from catalog")
        require(
            preview.get("catalog_version") == payload["catalog_version"],
            "inspect catalog version differs from the signed payload",
        )
        require(
            preview.get("archive_sha256") == payload["archive_sha256"],
            "inspect archive digest differs from the signed payload",
        )
    else:
        require(preview.get("trust") == DEVELOPER_TRUST, f"unsigned inspect trust is {preview.get('trust')!r}")
        require(
            preview.get("evidence_class") == DEVELOPER_EVIDENCE,
            "unsigned inspect claimed publisher evidence",
        )


def check_status(status: dict[str, Any], *, signed: bool, version: str) -> None:
    require(status.get("id") == EXTENSION_ID, "status reported a different extension")
    require(status.get("installed") is True, "extension is not installed after install")
    require(status.get("healthy") is True, f"installed extension is unhealthy: {status.get('detail')!r}")
    require(status.get("active_version") == version, f"active version {status.get('active_version')!r} != {version}")
    expected_trust = PRODUCTION_TRUST if signed else DEVELOPER_TRUST
    expected_evidence = PRODUCTION_EVIDENCE if signed else DEVELOPER_EVIDENCE
    require(status.get("trust") == expected_trust, f"installed trust is {status.get('trust')!r}")
    require(status.get("evidence_class") == expected_evidence, "installed evidence class differs")


def check_parse(result: dict[str, Any], *, version: str, image_sha256: str) -> None:
    require(result.get("schema") == VISUAL_REGIONS_SCHEMA, f"parse schema is {result.get('schema')!r}")
    parser = result.get("parser", {})
    require(parser.get("extension_id") == EXTENSION_ID, "parse came from a different extension")
    require(parser.get("extension_version") == version, "parse came from a different extension version")
    local_input = result.get("local_input", {})
    require(local_input.get("action_eligible") is False, "local image parse claimed action eligibility")
    require(local_input.get("action_authority") == "none", "local image parse claimed action authority")
    require(local_input.get("sha256") == image_sha256, "parse hashed a different image")
    regions = result.get("regions")
    require(isinstance(regions, list) and regions, "parse returned no regions for the fixture")
    for region in regions:
        bounds = region.get("bounds", {})
        require(
            all(isinstance(bounds.get(key), int) and bounds[key] >= 0 for key in ("x", "y", "width", "height")),
            f"region {region.get('id')!r} has invalid bounds",
        )
        confidence = region.get("confidence")
        require(
            isinstance(confidence, (int, float)) and 0 <= confidence <= 1,
            f"region {region.get('id')!r} has invalid confidence",
        )


def run_check(arguments: argparse.Namespace) -> dict[str, Any]:
    signed = arguments.catalog is not None
    payload = None
    if signed:
        payload = json.loads(arguments.catalog.read_text(encoding="utf-8"))["payload"]
        require(payload["version"] == arguments.version, "catalog version differs from the release")
        require(payload["target"] == arguments.target, "catalog target differs from the runner target")
        require(
            payload["corresponding_source_revision"] == arguments.source_sha,
            "catalog source revision differs from the release commit",
        )
        archive = arguments.catalog.parent / payload["archive"]
        require(archive.is_file(), f"catalog archive is missing beside the catalog: {payload['archive']}")
        require(file_sha256(archive) == payload["archive_sha256"], "catalog archive digest differs")
    require(not arguments.home.exists(), f"isolated Driver home already exists: {arguments.home}")
    arguments.home.mkdir(parents=True)
    capture = arguments.home.parent / f"{arguments.home.name}-capture.json"
    capture.write_text(
        json.dumps({"source": {"kind": "primary_desktop", "display_id": "primary"}}) + "\n",
        encoding="utf-8",
    )
    driver = Driver(arguments.driver, arguments.home)
    source = source_arguments(arguments.catalog, arguments.unsigned_archive)
    image_sha256 = file_sha256(arguments.image)

    version_output = driver.run(["--version"]).stdout.strip()
    check_absent(driver, arguments.image, capture)
    preview = driver.json(["extension", "inspect", EXTENSION_ID, *source, "--json"])
    check_inspect(
        preview, signed=signed, version=arguments.version, target=arguments.target,
        source_sha=arguments.source_sha, payload=payload,
    )
    driver.run(["extension", "install", EXTENSION_ID, *source])
    status = driver.json(["extension", "status", EXTENSION_ID, "--self-test", "--json"])
    check_status(status, signed=signed, version=arguments.version)
    parse = driver.json([
        "perception", "parse", "--image", str(arguments.image), "--capture", str(capture), "--json",
    ])
    check_parse(parse, version=arguments.version, image_sha256=image_sha256)
    driver.run(["extension", "remove", EXTENSION_ID])
    removed = driver.json(["extension", "status", EXTENSION_ID, "--json"])
    require(removed.get("installed") is False, "extension is still installed after remove")

    return {
        "driver_version": version_output,
        "extension_version": arguments.version,
        "target": arguments.target,
        "source_sha": arguments.source_sha,
        "trust": status["trust"],
        "evidence_class": status["evidence_class"],
        "catalog_version": preview.get("catalog_version"),
        "archive_sha256": preview.get("archive_sha256"),
        "parse_region_count": len(parse["regions"]),
        "parse_timing": parse.get("timing"),
        "checks": [
            "absent-not-installed",
            "inspect-without-mutation",
            "install",
            "status-self-test-healthy",
            "real-parse",
            "remove",
        ],
    }


def parse_arguments(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--driver", required=True, type=Path)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--catalog", type=Path, help="signed catalog beside its archive")
    source.add_argument(
        "--unsigned-archive", type=Path,
        help="developer-only unsigned archive for pull request dry runs",
    )
    parser.add_argument("--version", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--home", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    arguments = parse_arguments(sys.argv[1:] if argv is None else argv)
    try:
        evidence = run_check(arguments)
    except InstallCheckError as error:
        print(f"::error title=Cua Perception install check::{error}", file=sys.stderr)
        return 1
    arguments.evidence.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(evidence, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
