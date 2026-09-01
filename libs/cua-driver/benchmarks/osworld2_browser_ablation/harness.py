#!/usr/bin/env python3
"""Prepare and plan the Cua Driver OSWorld 2 browser-use ablation.

This module intentionally performs no Fleet mutation. It prepares the public,
release-pinned OSWorld checkout, validates the release contract, reports the
credential boundary, and builds a deterministic episode matrix after the
gated task files have been downloaded.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence


ROOT = Path(__file__).resolve().parent
DEFAULT_MANIFEST = ROOT / "manifest.json"
DEFAULT_WORK_DIR = ROOT / ".work"
DEFAULT_OSWORLD_DIR = DEFAULT_WORK_DIR / "OSWorld-V2"
DEFAULT_LOCAL_CONFIG = DEFAULT_WORK_DIR / "local.json"

BLOCKED_EXIT = 2


class HarnessError(RuntimeError):
    """A deterministic setup or validation failure."""


@dataclass(frozen=True)
class ModePolicy:
    native_screenshot: bool
    native_accessibility: bool
    browser_semantics: bool
    native_pixel_actions: bool
    native_accessibility_actions: bool
    browser_actions: bool


MODE_POLICIES: dict[str, ModePolicy] = {
    "screenshot_only": ModePolicy(
        native_screenshot=True,
        native_accessibility=False,
        browser_semantics=False,
        native_pixel_actions=True,
        native_accessibility_actions=False,
        browser_actions=False,
    ),
    "screenshot_ax": ModePolicy(
        native_screenshot=True,
        native_accessibility=True,
        browser_semantics=False,
        native_pixel_actions=True,
        native_accessibility_actions=True,
        browser_actions=False,
    ),
    "cdp_only": ModePolicy(
        native_screenshot=False,
        native_accessibility=False,
        browser_semantics=True,
        native_pixel_actions=False,
        native_accessibility_actions=False,
        browser_actions=True,
    ),
    "combined": ModePolicy(
        native_screenshot=True,
        native_accessibility=True,
        browser_semantics=True,
        native_pixel_actions=True,
        native_accessibility_actions=True,
        browser_actions=True,
    ),
}

NATIVE_PIXEL_TOOLS = (
    "click",
    "double_click",
    "right_click",
    "drag",
    "scroll",
    "type_text",
    "press_key",
    "hotkey",
)
NATIVE_AX_TOOLS = NATIVE_PIXEL_TOOLS + ("set_value",)
BROWSER_TOOLS = (
    "browser_navigate",
    "browser_click",
    "browser_type",
    "browser_pointer",
    "browser_dialog",
    "browser_set_input_files",
)


def read_json_object(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise HarnessError(f"missing JSON file: {path}") from exc
    except json.JSONDecodeError as exc:
        raise HarnessError(f"invalid JSON in {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise HarnessError(f"{path} must contain a JSON object")
    return value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run(
    command: Sequence[str],
    *,
    cwd: Path | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        list(command),
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if check and result.returncode:
        detail = (result.stderr or result.stdout).strip()
        raise HarnessError(f"{' '.join(command[:3])} failed: {detail[:2000]}")
    return result


def require_command(name: str) -> str:
    path = shutil.which(name)
    if not path:
        raise HarnessError(f"required command is not installed: {name}")
    return path


def git_output(checkout: Path, *args: str) -> str:
    return run(["git", "-C", str(checkout), *args]).stdout.strip()


def prepare_public_checkout(
    manifest: Mapping[str, Any],
    checkout: Path,
) -> dict[str, Any]:
    require_command("git")
    source = manifest["osworld_code"]
    checkout.parent.mkdir(parents=True, exist_ok=True)
    if not (checkout / ".git").exists():
        if checkout.exists() and any(checkout.iterdir()):
            raise HarnessError(f"refusing to clone over non-empty non-Git directory: {checkout}")
        run(
            [
                "git",
                "clone",
                "--filter=blob:none",
                "--no-checkout",
                source["repository"],
                str(checkout),
            ]
        )
    actual_origin = git_output(checkout, "remote", "get-url", "origin")
    accepted_origins = {
        source["repository"],
        source["repository"].removesuffix(".git"),
    }
    if actual_origin not in accepted_origins:
        raise HarnessError(
            f"OSWorld checkout origin mismatch: expected {source['repository']}, "
            f"found {actual_origin}"
        )
    run(
        [
            "git",
            "-C",
            str(checkout),
            "fetch",
            "--force",
            "origin",
            (f"{source['bootstrap_branch']}:refs/remotes/origin/{source['bootstrap_branch']}"),
            f"refs/tags/{source['tag']}:refs/tags/{source['tag']}",
        ]
    )
    actual_bootstrap_commit = git_output(
        checkout, "rev-parse", f"{source['bootstrap_commit']}^{{commit}}"
    )
    if actual_bootstrap_commit != source["bootstrap_commit"]:
        raise HarnessError(
            "OSWorld bootstrap commit mismatch: "
            f"expected {source['bootstrap_commit']}, found {actual_bootstrap_commit}"
        )
    actual_tag_commit = git_output(checkout, "rev-parse", f"{source['tag']}^{{commit}}")
    if actual_tag_commit != source["commit"]:
        raise HarnessError(
            f"OSWorld tag moved: expected {source['commit']}, found {actual_tag_commit}"
        )
    run(
        [
            "git",
            "-C",
            str(checkout),
            "checkout",
            "--detach",
            source["bootstrap_commit"],
        ]
    )
    validate_release_manifest(manifest, checkout)
    return {
        "path": str(checkout),
        "origin": actual_origin,
        "tag": source["tag"],
        "bootstrap_commit": actual_bootstrap_commit,
        "release_commit": actual_tag_commit,
        "head": actual_bootstrap_commit,
        "phase": "gated_download",
        "detached": True,
    }


def validate_release_manifest(
    manifest: Mapping[str, Any],
    checkout: Path,
) -> dict[str, Any]:
    release_name = manifest["benchmark_release"]
    official_path = checkout / "benchmark_releases" / f"{release_name}.json"
    official = read_json_object(official_path)

    expected_pairs = {
        ("release",): release_name,
        ("osworld_code", "tag"): manifest["osworld_code"]["tag"],
        ("tasks", "repository"): manifest["tasks"]["repository"],
        ("tasks", "tag"): manifest["tasks"]["revision"],
        ("assets", "repository"): manifest["assets"]["repository"],
        ("assets", "tag"): manifest["assets"]["revision"],
        ("task_hash_manifest", "repository"): manifest["tasks"]["repository"],
        ("task_hash_manifest", "tag"): manifest["tasks"]["revision"],
        ("task_hash_manifest", "path"): manifest["task_hash_manifest"]["path"],
        (
            "task_hash_manifest",
            "sha256",
        ): f"sha256:{manifest['task_hash_manifest']['sha256']}",
        ("task_hash_manifest", "task_count"): manifest["tasks"]["expected_count"],
        (
            "provider_images",
            "docker",
            "ubuntu",
            "artifact_path",
        ): manifest["image"]["archive"],
        (
            "provider_images",
            "docker",
            "ubuntu",
            "artifact_size",
        ): manifest["image"]["archive_size"],
        (
            "provider_images",
            "docker",
            "ubuntu",
            "artifact_sha256",
        ): f"sha256:{manifest['image']['archive_sha256']}",
    }
    mismatches: list[str] = []
    for path, expected in expected_pairs.items():
        value: Any = official
        for key in path:
            value = value.get(key) if isinstance(value, dict) else None
        if value != expected:
            mismatches.append(f"{'.'.join(path)} expected {expected!r}, found {value!r}")
    if mismatches:
        raise HarnessError("release manifest mismatch: " + "; ".join(mismatches))
    return {
        "path": str(official_path),
        "release": release_name,
        "task_count": manifest["tasks"]["expected_count"],
        "task_hash_manifest_sha256": manifest["task_hash_manifest"]["sha256"],
        "image_sha256": manifest["image"]["archive_sha256"],
    }


def load_local_config(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    return read_json_object(path)


def first_nonempty(env: Mapping[str, str], names: Iterable[str]) -> str | None:
    for name in names:
        value = env.get(name)
        if value:
            return value
    return None


def credential_status(
    env: Mapping[str, str],
    *,
    local_config: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    local_config = local_config or {}
    fleet_env = bool(env.get("CUA_CLIENT_ID") and env.get("CUA_CLIENT_SECRET"))
    fleet_secret = env.get(
        "CUA_FLEET_SECRET_NAME",
        str(local_config.get("fleet_secret_name") or ""),
    )
    container_disk_image = str(local_config.get("container_disk_image") or "")
    hf_token = first_nonempty(env, ("HF_TOKEN", "HUGGING_FACE_HUB_TOKEN"))
    provider = (
        (env.get("CUA_BENCH_MODEL_PROVIDER") or str(local_config.get("model_provider") or ""))
        .strip()
        .lower()
    )
    model_name = (env.get("CUA_BENCH_MODEL") or str(local_config.get("model") or "")).strip()
    model_base_url = (
        env.get("CUA_BENCH_OPENAI_BASE_URL") or str(local_config.get("model_base_url") or "")
    ).strip()
    model_secret_name = str(local_config.get("model_api_key_secret_name") or "")
    model_route_verified = (
        env.get("CUA_BENCH_MODEL_ROUTE_VERIFIED") == "1"
        or local_config.get("model_route_verified") is True
    )
    model_key = None
    if provider == "openai":
        model_key = env.get("OPENAI_API_KEY")
    elif provider == "anthropic":
        model_key = env.get("ANTHROPIC_API_KEY")
    elif provider == "gateway":
        model_key = env.get("CUA_MODEL_GATEWAY_TOKEN")
    elif provider == "litellm":
        model_key = first_nonempty(
            env,
            ("LITELLM_API_KEY", "CUA_MODEL_GATEWAY_TOKEN"),
        )
    model_secret_configured = bool(provider in {"gateway", "litellm"} and model_secret_name)

    missing: list[str] = []
    if not hf_token:
        missing.append("HF_TOKEN")
    if not provider:
        missing.append("CUA_BENCH_MODEL_PROVIDER")
    if not model_name:
        missing.append("CUA_BENCH_MODEL")
    supported_providers = {"openai", "anthropic", "gateway", "litellm"}
    if provider and provider not in supported_providers:
        missing.append("supported model provider")
    if provider in {"gateway", "litellm"} and not model_base_url:
        missing.append("CUA_BENCH_OPENAI_BASE_URL")
    if provider in {"gateway", "litellm"} and not model_route_verified:
        missing.append("verified model route")
    if provider in supported_providers and not model_key and not model_secret_configured:
        missing.append(
            {
                "openai": "OPENAI_API_KEY",
                "anthropic": "ANTHROPIC_API_KEY",
                "gateway": "CUA_MODEL_GATEWAY_TOKEN",
                "litellm": "LITELLM_API_KEY",
            }.get(provider, "supported model provider")
        )
    if not fleet_env and not fleet_secret:
        missing.append("Fleet credentials or CUA_FLEET_SECRET_NAME")
    if not container_disk_image:
        missing.append("container_disk_image in local config")

    return {
        "infrastructure": {
            "container_disk_image_configured": bool(container_disk_image),
        },
        "fleet": {
            "configured": fleet_env or bool(fleet_secret),
            "source": "environment" if fleet_env else ("aws_secret" if fleet_secret else None),
        },
        "hugging_face": {
            "configured": bool(hf_token),
            "required_repositories": [
                "xlangai/osworld_v2_tasks",
                "xlangai/osworld_v2_assets_gated",
            ],
        },
        "model": {
            "configured": bool(
                provider
                and provider in supported_providers
                and model_name
                and (provider not in {"gateway", "litellm"} or model_base_url)
                and (model_key or model_secret_configured)
                and (provider not in {"gateway", "litellm"} or model_route_verified)
            ),
            "provider": provider or None,
            "model": model_name or None,
            "openai_base_url_configured": bool(model_base_url),
            "route_verified": model_route_verified,
            "credential_source": (
                "environment" if model_key else ("aws_secret" if model_secret_configured else None)
            ),
        },
        "missing": missing,
        "ready_for_gated_download": bool(hf_token),
        "ready_for_model_run": not missing,
    }


def project_observation(
    mode: str,
    *,
    native: Mapping[str, Any] | None,
    browser: Mapping[str, Any] | None,
) -> dict[str, Any]:
    try:
        policy = MODE_POLICIES[mode]
    except KeyError as exc:
        raise HarnessError(f"unknown ablation mode: {mode}") from exc
    native = native or {}
    browser = browser or {}
    result: dict[str, Any] = {"mode": mode}
    if policy.native_screenshot:
        result["native_screenshot"] = native.get("screenshot")
        result["native_screenshot_metadata"] = native.get("screenshot_metadata")
    if policy.native_accessibility:
        result["native_accessibility"] = native.get("accessibility")
    if policy.browser_semantics:
        result["browser_outline"] = browser.get("outline")
        result["browser_refs"] = browser.get("refs")
        result["browser_snapshot"] = browser.get("snapshot")
    return result


def allowed_tools(mode: str) -> tuple[str, ...]:
    try:
        policy = MODE_POLICIES[mode]
    except KeyError as exc:
        raise HarnessError(f"unknown ablation mode: {mode}") from exc
    tools: list[str] = []
    if policy.native_pixel_actions:
        tools.extend(NATIVE_PIXEL_TOOLS)
    if policy.native_accessibility_actions:
        tools.extend(NATIVE_AX_TOOLS)
    if policy.browser_actions:
        tools.extend(BROWSER_TOOLS)
    return tuple(dict.fromkeys(tools))


def select_active_tab(tabs: Sequence[Mapping[str, Any]]) -> Mapping[str, Any]:
    active = [tab for tab in tabs if tab.get("active") is True]
    if len(active) != 1:
        raise HarnessError(f"expected exactly one proven active browser tab, found {len(active)}")
    return active[0]


def normalize_task_id(value: str) -> str:
    value = value.strip()
    if value.endswith(".py"):
        value = value[:-3]
    if not value.startswith("task_"):
        raise HarnessError(f"invalid OSWorld task id: {value!r}")
    suffix = value.removeprefix("task_")
    if not suffix or any(not (char.isalnum() or char == "_") for char in suffix):
        raise HarnessError(f"invalid OSWorld task id: {value!r}")
    return value


def downloaded_tasks(checkout: Path) -> set[str]:
    directory = checkout / "evaluation_examples" / "task_class"
    return {path.stem for path in directory.glob("task_*.py")}


def build_matrix(
    manifest: Mapping[str, Any],
    task_ids: Sequence[str],
    *,
    modes: Sequence[str] | None = None,
    seeds: Sequence[int] | None = None,
    model_provider: str,
    model: str,
    model_metadata: Mapping[str, Any] | None = None,
) -> list[dict[str, Any]]:
    normalized = [normalize_task_id(value) for value in task_ids]
    if len(set(normalized)) != len(normalized):
        raise HarnessError("task list contains duplicates")
    selected_modes = list(modes or manifest["pilot"]["modes"])
    selected_seeds = list(seeds or manifest["pilot"]["seeds"])
    model_metadata = model_metadata or {}
    unknown = sorted(set(selected_modes) - set(MODE_POLICIES))
    if unknown:
        raise HarnessError(f"unknown ablation modes: {unknown}")
    matrix = []
    for task_id in normalized:
        for mode in selected_modes:
            for seed in selected_seeds:
                matrix.append(
                    {
                        "episode_id": f"{task_id}__{mode}__seed-{seed}",
                        "task_id": task_id,
                        "mode": mode,
                        "seed": seed,
                        "model_provider": model_provider,
                        "model": model,
                        "model_wire_api": model_metadata.get("model_wire_api"),
                        "model_stream": model_metadata.get("model_stream"),
                        "model_resolved_at_smoke": model_metadata.get("model_resolved_at_smoke"),
                        "benchmark_release": manifest["benchmark_release"],
                        "osworld_code_commit": manifest["osworld_code"]["commit"],
                        "image_archive_sha256": manifest["image"]["archive_sha256"],
                    }
                )
    return matrix


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def command_prepare(args: argparse.Namespace) -> int:
    manifest = read_json_object(args.manifest)
    require_command("uv")
    checkout = args.work_dir / "OSWorld-V2"
    checkout_status = prepare_public_checkout(manifest, checkout)
    local_config = load_local_config(args.local_config)
    credentials = credential_status(os.environ, local_config=local_config)
    status = {
        "status": "ready" if not credentials["missing"] else "blocked_credentials",
        "public_setup": {
            "osworld_checkout": checkout_status,
            "release_manifest": validate_release_manifest(manifest, checkout),
            "ablation_modes": {
                name: {
                    **policy.__dict__,
                    "allowed_tools": allowed_tools(name),
                }
                for name, policy in MODE_POLICIES.items()
            },
        },
        "credentials": credentials,
        "next_step": (
            "Run the official gated task and asset download scripts, then build the "
            "sealed pilot matrix."
            if credentials["ready_for_gated_download"]
            else "Provide gated Hugging Face access; no Fleet VM has been created."
        ),
    }
    args.work_dir.mkdir(parents=True, exist_ok=True)
    write_json(args.work_dir / "status.json", status)
    print(json.dumps(status, indent=2))
    return 0 if status["status"] == "ready" else BLOCKED_EXIT


def command_preflight(args: argparse.Namespace) -> int:
    manifest = read_json_object(args.manifest)
    checkout = args.work_dir / "OSWorld-V2"
    if not (checkout / ".git").exists():
        raise HarnessError(f"public OSWorld checkout is not prepared: {checkout}")
    head = git_output(checkout, "rev-parse", "HEAD")
    legal_heads = {
        manifest["osworld_code"]["bootstrap_commit"]: "gated_download",
        manifest["osworld_code"]["commit"]: "benchmark",
    }
    if head not in legal_heads:
        raise HarnessError(f"OSWorld checkout is not pinned to a legal phase: {head}")
    checkout_status = {
        "path": str(checkout),
        "head": head,
        "phase": legal_heads[head],
        "bootstrap_commit": manifest["osworld_code"]["bootstrap_commit"],
        "release_commit": manifest["osworld_code"]["commit"],
    }
    release_status = validate_release_manifest(manifest, checkout)
    credentials = credential_status(
        os.environ,
        local_config=load_local_config(args.local_config),
    )
    payload = {
        "status": "ready" if not credentials["missing"] else "blocked_credentials",
        "checkout": checkout_status,
        "release": release_status,
        "credentials": credentials,
        "fleet_mutation_performed": False,
    }
    print(json.dumps(payload, indent=2))
    return 0 if payload["status"] == "ready" else BLOCKED_EXIT


def command_finalize(args: argparse.Namespace) -> int:
    manifest = read_json_object(args.manifest)
    checkout = args.work_dir / "OSWorld-V2"
    if not (checkout / ".git").exists():
        raise HarnessError(f"public OSWorld checkout is not prepared: {checkout}")

    available = downloaded_tasks(checkout)
    expected_count = manifest["tasks"]["expected_count"]
    if len(available) != expected_count:
        raise HarnessError(
            f"expected {expected_count} gated task files, found {len(available)}; "
            "finish the official task download first"
        )

    task_hash_manifest = (
        checkout / "cache" / "osworld_v2_tasks_metadata" / manifest["task_hash_manifest"]["path"]
    )
    if not task_hash_manifest.is_file():
        raise HarnessError(
            f"gated task hash manifest is missing: {task_hash_manifest}; "
            "finish the official metadata download first"
        )
    actual_task_hash_sha256 = sha256_file(task_hash_manifest)
    expected_task_hash_sha256 = manifest["task_hash_manifest"]["sha256"]
    if actual_task_hash_sha256 != expected_task_hash_sha256:
        raise HarnessError(
            "gated task hash manifest mismatch: expected "
            f"{expected_task_hash_sha256}, found {actual_task_hash_sha256}"
        )

    assets_dir = checkout / "cache" / "osworld_v2_assets"
    if not assets_dir.is_dir() or not any(path.is_file() for path in assets_dir.rglob("*")):
        raise HarnessError(
            f"gated asset cache is missing or empty: {assets_dir}; "
            "finish the official asset download first"
        )

    release_commit = manifest["osworld_code"]["commit"]
    run(["git", "-C", str(checkout), "checkout", "--detach", release_commit])
    head = git_output(checkout, "rev-parse", "HEAD")
    if head != release_commit:
        raise HarnessError(f"failed to finalize OSWorld checkout: {head} != {release_commit}")
    release_status = validate_release_manifest(manifest, checkout)
    payload = {
        "status": "ready",
        "phase": "benchmark",
        "checkout": str(checkout),
        "head": head,
        "gated_task_files": len(available),
        "task_hash_manifest_sha256": actual_task_hash_sha256,
        "assets_dir": str(assets_dir),
        "release": release_status,
        "next_command": f"cd {checkout} && uv sync --frozen",
        "fleet_mutation_performed": False,
    }
    print(json.dumps(payload, indent=2))
    return 0


def command_matrix(args: argparse.Namespace) -> int:
    manifest = read_json_object(args.manifest)
    checkout = args.work_dir / "OSWorld-V2"
    available = downloaded_tasks(checkout)
    if len(available) != manifest["tasks"]["expected_count"]:
        raise HarnessError(
            f"expected {manifest['tasks']['expected_count']} gated task files, "
            f"found {len(available)}; finish the official download first"
        )
    requested = [
        line.strip()
        for line in args.tasks.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    ]
    normalized = [normalize_task_id(value) for value in requested]
    missing = sorted(set(normalized) - available)
    if missing:
        raise HarnessError(f"selected tasks are not present in the pinned bundle: {missing}")
    if len(normalized) != manifest["pilot"]["task_count"]:
        raise HarnessError(
            f"pilot requires exactly {manifest['pilot']['task_count']} tasks, "
            f"found {len(normalized)}"
        )
    local_config = load_local_config(args.local_config)
    provider = os.environ.get(
        "CUA_BENCH_MODEL_PROVIDER",
        str(local_config.get("model_provider") or ""),
    )
    model = os.environ.get(
        "CUA_BENCH_MODEL",
        str(local_config.get("model") or ""),
    )
    matrix = build_matrix(
        manifest,
        normalized,
        model_provider=provider,
        model=model,
        model_metadata=local_config,
    )
    if len(matrix) != manifest["pilot"]["expected_episodes"]:
        raise HarnessError(
            f"episode count mismatch: expected {manifest['pilot']['expected_episodes']}, "
            f"found {len(matrix)}"
        )
    write_json(args.output, {"episodes": matrix})
    print(
        json.dumps(
            {
                "status": "ready",
                "tasks": len(normalized),
                "modes": len(manifest["pilot"]["modes"]),
                "seeds": len(manifest["pilot"]["seeds"]),
                "episodes": len(matrix),
                "output": str(args.output),
            },
            indent=2,
        )
    )
    return 0


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description="Prepare the Cua Driver OSWorld 2 browser-use ablation."
    )
    result.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    result.add_argument("--work-dir", type=Path, default=DEFAULT_WORK_DIR)
    result.add_argument("--local-config", type=Path, default=DEFAULT_LOCAL_CONFIG)
    subparsers = result.add_subparsers(dest="command", required=True)
    prepare = subparsers.add_parser(
        "prepare",
        help="Prepare and pin public inputs, then report the credential boundary.",
    )
    prepare.set_defaults(func=command_prepare)
    preflight = subparsers.add_parser(
        "preflight",
        help="Validate the prepared checkout and credentials without side effects.",
    )
    preflight.set_defaults(func=command_preflight)
    finalize = subparsers.add_parser(
        "finalize",
        help="Verify gated downloads and switch to the pinned evaluation commit.",
    )
    finalize.set_defaults(func=command_finalize)
    matrix = subparsers.add_parser(
        "matrix",
        help="Build the 20-task x 4-mode x 3-seed episode matrix.",
    )
    matrix.add_argument("--tasks", type=Path, required=True)
    matrix.add_argument(
        "--output",
        type=Path,
        default=ROOT / "results" / "pilot-matrix.json",
    )
    matrix.set_defaults(func=command_matrix)
    return result


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        return int(args.func(args))
    except HarnessError as exc:
        print(json.dumps({"status": "error", "message": str(exc)}, indent=2))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
