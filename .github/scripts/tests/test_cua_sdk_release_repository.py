"""The cua SDK / cua-spacesd / Cua Spaces release workflows run the same on
trycua/cua and on trycua/cua-staging.

Staging must build, sign what it can, and publish GitHub prereleases from its
own repository, while never reaching PyPI, npm or real registry names, and
trycua/cua must keep failing closed on missing signing secrets.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / ".github/workflows"
RELEASE_WORKFLOWS = ("cd-cua-sdk.yml", "cd-cua-spacesd.yml", "cd-cua-spaces.yml")
CANONICAL = "github.repository == 'trycua/cua'"


def text(name: str) -> str:
    return (WORKFLOWS / name).read_text()


def workflow(name: str) -> dict:
    return yaml.safe_load(text(name))


def steps(name: str, job: str) -> list[dict]:
    return workflow(name)["jobs"][job]["steps"]


def step(name: str, job: str, title: str) -> dict:
    matches = [s for s in steps(name, job) if s.get("name", "").startswith(title)]
    assert len(matches) == 1, f"{name}:{job}: {title!r} matched {len(matches)} steps"
    return matches[0]


@pytest.mark.parametrize("name", RELEASE_WORKFLOWS)
def test_release_urls_derive_from_the_repository(name: str) -> None:
    source = text(name)
    # No release URL, feed or identity is pinned to trycua/cua.
    assert "github.com/trycua/cua/" not in source
    assert "--repo trycua/cua" not in source
    assert CANONICAL in source


@pytest.mark.parametrize("name", ("cd-cua-sdk.yml", "cd-cua-spaces.yml"))
def test_published_installers_are_stamped_with_the_repository(name: str) -> None:
    source = text(name)
    assert 'stamp_installers.py --repo "$GITHUB_REPOSITORY"' in source
    assert "scripts/install/install.sh" not in source.split("stamp_installers.py", 1)[1]
    assert "stamped/install.sh stamped/install.ps1" in source


@pytest.mark.parametrize("name", ("cd-cua-sdk.yml", "cd-cua-spaces.yml"))
def test_the_cli_is_built_against_this_repository(name: str) -> None:
    assert workflow(name)["env"]["CUA_RELEASE_REPOSITORY"] == "${{ github.repository }}"


def test_spaces_builds_use_the_release_config_overlay() -> None:
    source = text("cd-cua-spaces.yml")
    code = [line for line in source.splitlines() if not line.lstrip().startswith("#")]
    assert not [line for line in code if "tauri.sidecar.conf.json" in line]
    # Tauri builds only Linux and Windows; macOS is the SwiftUI app.
    assert source.count("--config src-tauri/tauri.release.conf.json") == 3
    assert source.count('scripts/release-config.py --version "$VERSION" --repo "$GITHUB_REPOSITORY"') == 1
    mac = "\n".join(s.get("run", "") for s in steps("cd-cua-spaces.yml", "build-macos"))
    assert "tauri" not in mac
    assert "apps/cua-spaces-macos/scripts/build-release.sh" in mac


def test_spaces_signing_fails_closed_only_on_trycua_cua() -> None:
    wf = workflow("cd-cua-spaces.yml")
    assert wf["env"]["REQUIRE_SIGNING"] == (
        "${{ github.repository == 'trycua/cua' && startsWith(github.ref, 'refs/tags/') }}"
    )
    gate = step("cd-cua-spaces.yml", "build-macos", "Signing configuration")["run"]
    assert 'elif [ "$REQUIRE_SIGNING" = true ]; then' in gate
    assert "refusing to publish an unsigned Cua Spaces release" in gate
    # One build step: ad-hoc signed ("-") unless the Apple secrets are all set.
    build = step("cd-cua-spaces.yml", "build-macos", "Build and sign Cua Spaces.app")
    assert "if" not in build
    assert build["env"]["SIGNED"] == "${{ steps.signing.outputs.apple }}"
    assert 'identity="-"' in build["run"]
    assert '[ "$SIGNED" = true ] && identity="Developer ID Application:' in build["run"]
    assert not [k for k in build["env"] if k.startswith("APPLE_")]
    notarize = step("cd-cua-spaces.yml", "build-macos", "Notarize and staple the app")
    assert notarize["if"] == "steps.signing.outputs.apple == 'true'"
    windows = step("cd-cua-spaces.yml", "build-linux-windows", "Signing configuration")["run"]
    assert 'elif [ "$REQUIRE_SIGNING" = true ]; then' in windows
    verify = step("cd-cua-spaces.yml", "build-linux-windows", "Verify Authenticode")["run"]
    assert "$env:REQUIRE_SIGNING -eq 'true'" in verify


def test_spaces_releases_macos_only_for_now() -> None:
    jobs = workflow("cd-cua-spaces.yml")["jobs"]
    # The Tauri Linux/Windows job is kept but off unless explicitly enabled.
    assert jobs["build-linux-windows"]["if"] == "vars.CUA_SPACES_LINUX_WINDOWS == 'true'"
    assert "if" not in jobs["build-macos"]
    # Publishing runs with the Tauri job skipped, never after it failed.
    gate = jobs["installer-manifest"]["if"]
    assert "!cancelled()" in gate
    assert "needs.build-macos.result == 'success'" in gate
    assert "needs.build-linux-windows.result == 'skipped'" in gate
    assert "startsWith(github.ref, 'refs/tags/cua-spaces-v')" in gate
    # latest.json (the Tauri feed) only when Linux/Windows artifacts exist.
    sign = step("cd-cua-spaces.yml", "installer-manifest", "Sign, checksum and build release-artifacts.json")["run"]
    assert 'compgen -G "assets/cua-spaces-$VERSION-linux-*"' in sign
    assert "updater_feed.py" in sign
    assert "cosign sign-blob" in sign and "checksums.txt" in sign
    feeds = step("cd-cua-spaces.yml", "installer-manifest", "Update the rolling feeds")["run"]
    assert "if [ -f assets/latest.json ]; then" in feeds
    assert "cua-install-latest assets/release-artifacts.json" in feeds
    appcast = step("cd-cua-spaces.yml", "installer-manifest", "Upload the Sparkle appcast")
    assert appcast["if"] == "needs.build-macos.outputs.appcast == 'true'"


def test_spaces_prereleases_never_become_latest_or_move_canonical_feeds() -> None:
    publish = step("cd-cua-spaces.yml", "installer-manifest", "Publish the release")["run"]
    assert "--prerelease --latest=false" in publish
    assert "UNSIGNED" in publish
    feeds = step("cd-cua-spaces.yml", "installer-manifest", "Update the rolling feeds")
    assert feeds["if"] == "needs.build-macos.outputs.prerelease != 'true' || github.repository != 'trycua/cua'"


def test_sdk_registries_publish_only_from_trycua_cua() -> None:
    wf = workflow("cd-cua-sdk.yml")
    assert wf["env"]["CANONICAL"] == "${{ github.repository == 'trycua/cua' }}"
    pypi = step("cd-cua-sdk.yml", "publish", "Publish wheels to PyPI")["run"]
    upload = pypi.index("twine upload --skip-existing dist/wheel-*/*.whl")
    assert pypi.rindex('if [ "$CANONICAL" = true ]; then', 0, upload) >= 0
    assert "test.pypi.org" in pypi
    npm = step("cd-cua-sdk.yml", "publish", "Publish npm packages")["run"]
    assert "flags+=(--dry-run)" in npm
    assert "flags+=(--tag next)" in npm
    assert npm.count('npm publish "') == 2
    assert all('"${flags[@]}"' in line for line in npm.splitlines() if 'npm publish "' in line)


def test_sdk_signing_fails_closed_only_on_trycua_cua() -> None:
    native = workflow("cd-cua-sdk.yml")["jobs"]["native"]
    assert native["env"]["REQUIRE_SIGNING"] == (
        "${{ needs.version.outputs.publish == 'true' && github.repository == 'trycua/cua' }}"
    )
    mac = step("cd-cua-sdk.yml", "native", "Developer ID sign and notarize")["run"]
    assert 'if [ "$REQUIRE_SIGNING" = true ]; then' in mac
    win = step("cd-cua-sdk.yml", "native", "Verify Authenticode")["run"]
    assert "$env:REQUIRE_SIGNING -eq 'true'" in win


def test_spacesd_signing_fails_closed_only_on_trycua_cua() -> None:
    mac = workflow("cd-cua-spacesd.yml")["jobs"]["macos"]
    assert mac["env"]["REQUIRE_SIGNING"] == (
        "${{ needs.version.outputs.publish == 'true' && github.repository == 'trycua/cua' }}"
    )
    gate = step("cd-cua-spacesd.yml", "windows", "Require Authenticode")
    assert CANONICAL in gate["if"]


def test_driver_packages_publish_only_from_trycua_cua() -> None:
    source = text("cd-py-cua-driver.yml")
    assert workflow("cd-py-cua-driver.yml")["env"]["CANONICAL"] == "${{ github.repository == 'trycua/cua' }}"
    assert source.count("--access public --dry-run") == 2
    assert 'if [ "$CANONICAL" = true ]; then\n            python -m twine upload' in source


@pytest.mark.parametrize("name", ("cd-image-linux.yml", "cd-image-omarchy.yml", "cd-image-macos.yml"))
def test_images_never_push_off_trycua_cua(name: str) -> None:
    assert 'if [ "$publish" = true ] && [ "$GITHUB_REPOSITORY" != trycua/cua ]; then' in text(name)


def test_image_api_pushes_a_staging_package_off_trycua_cua() -> None:
    assert "PACKAGE=cua-image-api-staging" in text("cd-image-api.yml")


def test_docker_hub_pushes_only_from_trycua_cua() -> None:
    wf = workflow("docker-reusable-publish.yml")
    build = wf["jobs"]["build-and-push"]["steps"]
    pushes = [s for s in build if s.get("with", {}).get("push") is True]
    assert len(pushes) == 2
    assert all(CANONICAL in s["if"] for s in pushes)
    merge = [j for n, j in wf["jobs"].items() if n != "build-and-push"]
    assert all(CANONICAL in j["if"] for j in merge)


RELEASE_CONFIG = ROOT / "apps/cua-spaces/scripts/release-config.py"


def release_config(tmp_path: Path, version: str, repo: str) -> tuple[int, dict | None, str]:
    out = tmp_path / "conf.json"
    result = subprocess.run(
        [sys.executable, str(RELEASE_CONFIG), "--version", version, "--repo", repo, "--out", str(out)],
        capture_output=True,
        text=True,
    )
    return result.returncode, json.loads(out.read_text()) if out.exists() else None, result.stderr


def test_release_config_points_the_updater_at_the_building_repository(tmp_path: Path) -> None:
    code, conf, err = release_config(tmp_path, "0.2.0-staging.1", "trycua/cua-staging")
    assert code == 0, err
    assert conf["plugins"]["updater"]["endpoints"] == [
        "https://github.com/trycua/cua-staging/releases/download/cua-spaces-latest/latest.json"
    ]
    base = json.loads((ROOT / "apps/cua-spaces/src-tauri/tauri.conf.json").read_text())
    assert conf["plugins"]["updater"]["pubkey"] == base["plugins"]["updater"]["pubkey"]
    assert conf["bundle"]["externalBin"] == ["binaries/cua"]
    assert conf["version"] == "0.2.0-staging.1"
    assert conf["bundle"]["windows"]["wix"]["version"] == "0.2.0"


def test_release_config_requires_a_stable_tag_to_match_the_app_version(tmp_path: Path) -> None:
    base = json.loads((ROOT / "apps/cua-spaces/src-tauri/tauri.conf.json").read_text())
    code, conf, err = release_config(tmp_path, base["version"], "trycua/cua")
    assert code == 0, err
    assert conf["plugins"]["updater"]["endpoints"] == base["plugins"]["updater"]["endpoints"]
    assert "wix" not in conf["bundle"].get("windows", {})
    code, _, err = release_config(tmp_path / "x", "99.0.0", "trycua/cua")
    assert code == 1 and "does not match" in err
    for bad_version, bad_repo in (("1.2", "trycua/cua"), ("1.2.3", "trycua"), ("1.2.3", "a/b'c")):
        assert release_config(tmp_path, bad_version, bad_repo)[0] == 1
