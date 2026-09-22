from __future__ import annotations

import json
from pathlib import Path
import re

import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/cd-cua-perception-review-supplied-inputs.yml"
LOCK = ROOT / "libs/cua-driver/rust/crates/cua-perception/scripts/artifacts.lock.json"


def workflow_text() -> str:
    return WORKFLOW.read_text(encoding="utf-8")


def workflow_triggers(parsed: dict) -> dict:
    return parsed.get("on", parsed.get(True))


def test_review_pipeline_declares_equal_literal_trigger_inputs() -> None:
    text = workflow_text()
    parsed = yaml.safe_load(text)
    triggers = workflow_triggers(parsed)
    assert triggers["workflow_dispatch"]["inputs"] == triggers["workflow_call"]["inputs"]
    assert "&review_inputs" not in text
    assert "*review_inputs" not in text


def test_review_pipeline_is_valid_pinned_and_nonpublishing() -> None:
    text = workflow_text()
    parsed = yaml.safe_load(text)
    assert isinstance(parsed, dict)
    assert parsed["permissions"] == {
        "actions": "read",
        "contents": "read",
        "pull-requests": "read",
    }
    assert all(access != "write" for access in parsed["permissions"].values())
    assert "contents: write" not in text
    uses = re.findall(r"^\s*uses:\s*([^\s#]+)", text, flags=re.MULTILINE)
    assert uses
    assert all(re.fullmatch(r"[^@]+@[0-9a-f]{40}", use) for use in uses)
    lowered = text.lower()
    assert "gh release" not in lowered
    assert "--method" not in lowered
    assert "method=" not in lowered
    assert "data=" not in lowered
    assert "softprops/action-gh-release" not in lowered
    assert "actions/create-release" not in lowered
    assert "actions/upload-release-asset" not in lowered
    assert "perception_ed25519_private_key_base64" not in lowered
    assert "review_only: true" in lowered
    assert "environment:" not in text
    assert "secrets." not in text
    assert parsed["jobs"]["reviewed-model"]["permissions"] == {
        "actions": "read",
        "contents": "read",
        "pull-requests": "read",
    }
    assert parsed["jobs"]["supplied-input"]["permissions"] == {
        "actions": "read",
        "contents": "read",
        "pull-requests": "read",
    }
    assert parsed["jobs"]["aggregate"]["permissions"] == {
        "actions": "read",
        "contents": "read",
    }


def test_reviewed_model_broker_is_read_only_pinned_and_immutable() -> None:
    text = workflow_text()
    parsed = yaml.safe_load(text)
    broker = parsed["jobs"]["reviewed-model"]
    broker_text = str(broker)
    assert "actions/checkout" not in broker_text
    assert "libs/cua-driver" not in broker_text
    assert broker["outputs"] == {"source_asset_id": "${{ steps.verify.outputs.source_asset_id }}"}
    assert broker["permissions"] == {
        "actions": "read",
        "contents": "read",
        "pull-requests": "read",
    }
    assert "35438356263" in broker_text
    assert "10582583541" in broker_text
    assert "571471639" in broker_text
    assert "289372dea8b2b11f572f0f6a15824315bc74026b" in broker_text
    assert ".github/workflows/review-cua-perception-pr3943.yml" in broker_text
    assert '"$workflow_path" == "$expected_workflow"@*' in broker_text
    assert '[[ "$(jq -r .conclusion <<<"$run_json")" == "success" ]]' in broker_text
    assert '[[ "$(jq -r .expired <<<"$artifact_json")" == "false" ]]' in broker_text
    assert (
        '[[ "$(jq -r .workflow_run.id <<<"$artifact_json")" == "$REVIEWED_RUN_ID" ]]' in broker_text
    )
    assert (
        '[[ "$(jq -r .workflow_run.head_sha <<<"$artifact_json")" == "$REVIEWED_SOURCE_SHA" ]]'
        in broker_text
    )
    assert "omniparser-icon-detect-1280-opset17.onnx" in broker_text
    assert "d8a876bf7f9fb73d7da9432904ade7fa78e092e9a91674e5a2806b45562a9ab2" in broker_text
    assert "80_933_219" in broker_text
    producer_download = next(
        step
        for step in broker["steps"]
        if step.get("name") == "Download the exact reviewed producer artifact"
    )
    assert producer_download["with"] == {
        "artifact-ids": "10582583541",
        "github-token": "${{ github.token }}",
        "path": "reviewed-producer-input",
        "repository": "trycua/cua",
        "run-id": "35438356263",
    }
    assert "actions/upload-artifact@65c4c4a1ddee5b72f698fdd19549f0f0fb45cf08" in broker_text

    supplied = parsed["jobs"]["supplied-input"]
    assert supplied["needs"] == "reviewed-model"
    assert any(
        step.get("uses", "").startswith("actions/download-artifact@")
        and step["with"]
        == {
            "name": "reviewed-cua-perception-model",
            "path": "reviewed-inputs",
        }
        for step in supplied["steps"]
    )
    assert "${{ needs.reviewed-model.outputs.source_asset_id }}" in text
    assert "${{ needs.reviewed-model.outputs.asset_id }}" not in text
    for job in parsed["jobs"].values():
        if "actions/checkout@" in str(job):
            assert job["permissions"]["contents"] == "read"


def test_review_pipeline_binds_current_pr_head_and_authenticated_supplied_model() -> None:
    text = workflow_text()
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    detector = next(item for item in lock["artifacts"] if item["role"] == "icon-detect")
    assert detector["url"] is None
    assert detector["size"] == 80_933_219
    assert detector["sha256"] == "d8a876bf7f9fb73d7da9432904ade7fa78e092e9a91674e5a2806b45562a9ab2"
    assert "digest != expected_sha256" in text
    assert "refs/pull/$REQUESTED_PR_NUMBER/head" in text
    assert "pulls/$REQUESTED_PR_NUMBER" in text
    assert "actions/runs/$REVIEWED_RUN_ID" in text
    assert "actions/artifacts/$REVIEWED_ARTIFACT_ID" in text
    assert "browser_download_url" not in text
    assert text.index("differ from the reviewed size or SHA-256") < text.index(
        "Generate an ephemeral review trust root"
    )
    assert 'test "$EVENT_HEAD_SHA" = "$REQUESTED_SHA"' in text
    assert 'test "$GITHUB_SHA" = "$REQUESTED_SHA"' in text
    assert 'test "$EVENT_PR_NUMBER" = "$REQUESTED_PR_NUMBER"' in text
    assert 'test "$EVENT_LABEL" = cua-perception-live-review' in text
    assert '"$GITHUB_EVENT_NAME" == pull_request' in text
    assert '"$REQUESTED_PR_NUMBER" =~ ^[1-9][0-9]*$' in text


def test_review_pipeline_builds_pending_trust_override_and_exact_artifact_contracts() -> None:
    text = workflow_text()
    parsed = yaml.safe_load(text)
    supplied_input = parsed["jobs"]["supplied-input"]
    assert supplied_input["env"] == {
        "MSYS2_ENV_CONV_EXCL": "CUA_DRIVER_REVIEW_EXTENSION_PUBLIC_KEY_BASE64"
    }
    for value in (
        "CUA_DRIVER_REVIEW_EXTENSION_PUBLIC_KEY_BASE64",
        "--features review-trust-root",
        'publisher_id = "cua-review-only"',
        'publisher_name = "Cua REVIEW ONLY"',
        'key_id = "review-only-build-override"',
        'signature_algorithm: "ed25519"',
        "cua-perception-input-linux-x86_64",
        "cua-perception-input-windows-x86_64",
        "cua-perception-input-macos-aarch64",
        '("windows", "x86_64-pc-windows-msvc")',
        '("linux-x11", "x86_64-unknown-linux-gnu")',
        '("macos", "aarch64-apple-darwin")',
        "STAGING-cua-perception-review-candidates-",
        "review_driver_relative_path: driverName",
        'review_driver_build_profile: "debug-review-trust-root"',
    ):
        assert value in text
    driver_build = re.search(
        r"cargo build --manifest-path libs/cua-driver/rust/Cargo.toml \\\n+\s+-p cua-driver[^\n]+",
        text,
    )
    assert driver_build and "--release" not in driver_build.group(0)
    assert "const payloadBytes = Buffer.from(JSON.stringify(payload));" in text
    assert "crypto.sign(null, payloadBytes, privateKey)" in text
    assert "crypto.verify(null, payloadBytes, key" in text
    assert "fs.unlinkSync(privatePath)" in text
    aggregate = parsed["jobs"]["aggregate"]
    assert aggregate["outputs"] == {
        "signed_candidate_artifact_id": "${{ steps.upload.outputs.artifact-id }}",
        "producer_run_id": "${{ steps.producer.outputs.run_id }}",
    }
    download = next(
        step
        for step in aggregate["steps"]
        if step.get("name") == "Download all signed target layouts"
    )
    assert download["with"] == {
        "pattern": "review-cua-perception-*-${{ inputs.source_sha }}",
        "path": "staged",
        "merge-multiple": True,
    }
    upload = next(step for step in aggregate["steps"] if step.get("id") == "upload")
    assert upload["with"]["name"] == (
        "STAGING-cua-perception-review-candidates-${{ inputs.source_sha }}"
    )
    assert upload["with"]["path"] == "staged/"


def test_review_pipeline_preserves_the_assembler_candidate_manifest() -> None:
    parsed = yaml.safe_load(workflow_text())
    conversion = next(
        step["run"]
        for step in parsed["jobs"]["supplied-input"]["steps"]
        if step.get("name") == "Convert sealed assembler evidence to candidate input"
    )
    assert 'manifest = json.loads((root / "artifact-manifest.json").read_text())' in conversion
    assert '(root / "release-input.json").write_text(' in conversion
    assert '"verification-report"' not in conversion
    assert 'artifact["kind"] = "supplied-verification-report"' not in conversion
    assert 'report["evidenceKind"] = "supplied"' not in conversion
    assert 'manifest.pop("verification", None)' not in conversion
    assert 'manifest["artifacts"].append' not in conversion
    assert '"kind": "model-manifest"' not in conversion
    assert '"model-manifest.json"' not in conversion
    assert "perception_release.py bind-source" in conversion
    assert "perception_release.py validate" in conversion
    assert 'for stale in ("artifact-manifest.json", "SHA256SUMS")' in conversion


def test_macos_review_candidate_uses_ephemeral_certificate_signing() -> None:
    text = workflow_text()
    script = (ROOT / ".github/scripts/macos-review-codesign.sh").read_text()
    assert "runner: macos-26" in text
    assert "runner: macos-15" not in text
    assert "Match the production Driver build and hosted desktop E2E image" in text
    assert "bash .github/scripts/macos-review-codesign.sh" in text
    assert "code_signing: codeSigning" in text
    for contract in (
        "mktemp -d",
        "security create-keychain",
        "extendedKeyUsage=codeSigning",
        "openssl pkcs12 -export -legacy",
        '-P "$identity_password" -A -T /usr/bin/codesign',
        'cp "$driver_path" "$signing_probe"',
        'identity_selector="$identity_name"',
        'codesign --force --sign "$identity_selector" "$signing_probe"',
        'run_step driver-sign codesign --force --sign "$identity_selector"',
        "run_step driver-verify codesign --verify --strict",
        "security delete-keychain",
        "trap cleanup EXIT INT TERM",
        'requirement_output="$({ codesign -d -r- "$driver_path"; } 2>&1)"',
        '"identity": "ephemeral-self-signed-review-only"',
    ):
        assert contract in script
    assert "Developer ID" not in script

    snapshot = script.index('previous_keychains_output="$(security list-keychains -d user)"')
    default_snapshot = script.index(
        'previous_default_keychain="$(security default-keychain -d user |'
    )
    create = script.index('security create-keychain -p "$keychain_password"')
    unlock = script.index('security unlock-keychain -p "$keychain_password"')
    prepend = script.index(
        'security list-keychains -d user -s "$keychain_path" "${previous_keychains[@]}"',
        unlock,
    )
    default_set = script.index('security default-keychain -d user -s "$keychain_path"', prepend)
    identity_import = script.index('security import "$identity_path"')
    identity_lookup = script.index("run_step identity security find-identity")
    sudo_preflight = script.index("run_step sudo-preflight sudo -n -v")
    cleanup_flag = script.index("admin_trust_cleanup_needed=true", sudo_preflight)
    add_trust = script.index("run_step add-trust sudo -n security add-trusted-cert")
    trusted_cert_present = script.index("run_step trusted-cert-present security find-certificate")
    valid_identity = script.index("run_step valid-identity security find-identity -v")
    partition_list = script.index("security set-key-partition-list")
    probe_sign = script.index('identity_selector="$identity_name"')
    driver_sign = script.index("run_step driver-sign codesign")
    assert (
        default_snapshot
        < snapshot
        < create
        < unlock
        < prepend
        < default_set
        < identity_import
        < identity_lookup
        < sudo_preflight
        < cleanup_flag
        < add_trust
        < trusted_cert_present
        < valid_identity
        < partition_list
        < probe_sign
        < driver_sign
    )
    assert script.count('previous_keychains_output="$(security list-keychains -d user)"') == 1
    assert script.count('previous_default_keychain="$(security default-keychain -d user |') == 1
    assert 'done <<< "$previous_keychains_output"' in script[snapshot:create]
    assert "search_list_snapshotted=true" in script[snapshot:create]
    assert 'if [[ "$search_list_snapshotted" == true ]]; then' in script
    assert (
        'security list-keychains -d user -s "${previous_keychains[@]}" >/dev/null 2>&1 || true'
    ) in script
    assert (
        'security default-keychain -d user -s "$previous_default_keychain" '
        "\\\n      >/dev/null 2>&1 || true"
    ) in script
    assert "set -x" not in script
    assert '--keychain "$keychain_path"' not in script[probe_sign:]
    assert '--keychain "$keychain_path" "$signing_probe"' not in script
    assert '--keychain "$keychain_path" "$driver_path"' not in script
    assert 'cp /bin/echo "$signing_probe"' not in script
    assert script.count("openssl pkcs12 -export") == 2
    assert '"$log_dir/pkcs12-legacy.log" "$log_dir/pkcs12-fallback.log"' in script
    assert 'emit_log "$log_dir/pkcs12-legacy.log"' in script
    assert 'emit_log "$log_dir/pkcs12-fallback.log"' in script
    assert "-keypbe PBE-SHA1-3DES" not in script
    assert "-certpbe PBE-SHA1-3DES" not in script
    assert "[[:xdigit:]]{64}" in script
    assert '"${RUNNER_ENVIRONMENT:-}" == github-hosted' in script
    assert '"${RUNNER_OS:-}" == macOS' in script
    assert '"${GITHUB_ACTIONS:-}" == true' in script
    assert 'security remove-trusted-cert -d "$certificate_path"' in script
    assert 'security delete-certificate -Z "$trusted_identity"' in script
    assert "security dump-trust-settings -d" in script
    assert "run_bounded security find-certificate -Z -a" in script
    assert 'exit "$cleanup_status"' in script
    assert 'exit "$status"' not in script
    trust_removal = script.index('security remove-trusted-cert -d "$certificate_path"')
    certificate_cleanup = script.index('rm -f "$private_key_path"')
    assert trust_removal < certificate_cleanup
    assert 'grep -Eqi "certificate (leaf|root) = H\\"$identity_hash\\""' in script
    assert 'rmdir "$work_root"' in script
    assert 'rm -rf "$work_root"' not in script

    for production_workflow in ("cd-rust-cua-driver.yml", "cd-swift-lume.yml"):
        production = (ROOT / ".github/workflows" / production_workflow).read_text()
        assert " -A " not in production


def test_review_measurements_are_extracted_from_the_sealed_archive() -> None:
    text = workflow_text()
    for member in (
        'member_bytes("metadata/artifact-manifest.json")',
        'member_bytes("extension.json")',
        'member_json("metadata/runtime-contract.json")',
        'member_json("model-manifest.json")',
        'member_json("modelLedger.json")',
        'member_bytes("metadata/executed-verification.json")',
    ):
        assert member in text
    assert 'for role in ("icon-detect", "ocr-detect", "ocr-recognize")' in text
    assert "digest(sealed_bytes)" in text
    assert 'gates.get("self-test", {}).get("status") != "passed"' in text
    assert '"sealed_artifact_manifest_sha256": digest(artifact_bytes)' in text
    assert '"sealed_extension_manifest_sha256": digest(extension_bytes)' in text
    assert '"review_driver_version": match.group(0)' in text
    assert (
        "modelLock.artifacts.find" in text
    )  # download identity only; sealed values replace it below
    assert text.index("measurements.update({") < text.rindex("signed-candidate-checksums.txt")


def test_review_pipeline_native_driver_inspects_the_final_candidate_without_installing() -> None:
    parsed = yaml.safe_load(workflow_text())
    steps = parsed["jobs"]["supplied-input"]["steps"]
    package_index = next(
        index
        for index, step in enumerate(steps)
        if step.get("name") == "Package, review-sign, and measure the candidate"
    )
    inspect_index = next(
        index
        for index, step in enumerate(steps)
        if step.get("name") == "Inspect the signed candidate with the native review Driver"
    )
    upload_index = next(
        index
        for index, step in enumerate(steps)
        if step.get("name") == "Preserve signed review-only target without publishing"
    )
    assert package_index < inspect_index < upload_index

    inspection = steps[inspect_index]["run"]
    assert 'CUA_DRIVER_RS_HOME="$inspect_home"' in inspection
    assert 'test ! -e "$inspect_home"' in inspection
    assert 'extension inspect cua-perception --catalog "$catalog" --json' in inspection
    assert "extension install" not in inspection
    assert "extension update" not in inspection
    assert 'preview["trust"] == "review-only-publisher-verified"' in inspection
    assert 'preview["publisher_signature_verified"] is True' in inspection
    assert 'preview["archive_sha256"] == measurements["archive_sha256"]' in inspection
    assert (
        'hashlib.sha256(archive.read_bytes()).hexdigest() == preview["archive_sha256"]'
        in inspection
    )
    assert 'model["license_file"]' in inspection
    assert 'component["notice_file"]' in inspection
    assert 'source = preview["corresponding_source"]' in inspection
    assert 'preview["mutation_performed"] is False' in inspection
    assert 'preview["installed"] is False' in inspection


def test_review_pipeline_installs_and_self_tests_each_packaged_candidate() -> None:
    parsed = yaml.safe_load(workflow_text())
    steps = parsed["jobs"]["supplied-input"]["steps"]
    inspect_index = next(
        index
        for index, step in enumerate(steps)
        if step.get("name") == "Inspect the signed candidate with the native review Driver"
    )
    install_index = next(
        index
        for index, step in enumerate(steps)
        if step.get("name")
        == "Install and self-test the packaged candidate with the native review Driver"
    )
    cleanup_index = next(
        index
        for index, step in enumerate(steps)
        if step.get("name") == "Destroy any remaining ephemeral private key"
    )
    upload_index = next(
        index
        for index, step in enumerate(steps)
        if step.get("name") == "Preserve signed review-only target without publishing"
    )
    assert inspect_index < install_index < cleanup_index < upload_index

    self_test = steps[install_index]
    assert self_test["shell"] == "bash"
    assert self_test["env"] == {"PLATFORM": "${{ matrix.platform }}"}
    run = self_test["run"]
    assert 'driver="$root/review-${{ matrix.driver }}"' in run
    assert 'catalog="$root/signed-catalog.json"' in run
    assert 'test ! -e "$install_home"' in run
    absent_check = run.index('test ! -e "$install_home"')
    first_driver_call = run.index('CUA_DRIVER_RS_HOME="$install_home"', absent_check)
    assert run[absent_check:first_driver_call].strip() == 'test ! -e "$install_home"'
    assert 'mkdir "$install_home"' not in run
    assert run.count('CUA_DRIVER_RS_HOME="$install_home"') == 2
    assert 'extension install cua-perception --catalog "$catalog"' in run
    assert 'extension status cua-perception --self-test --json > "$status"' in run
    for contract in (
        'installed["installed"] is True',
        'installed["healthy"] is True',
        'installed["trust"] == "review-only-publisher-verified"',
        'installed["evidence_class"] == "review-only-not-release-evidence"',
        'installed["publisher_id"] == measurements["publisher_id"] == "cua-review-only"',
        'installed["publisher_key_id"] == measurements["key_id"] == "review-only-build-override"',
        'installed["active_version"] == measurements["extension_version"]',
        'installed["protocol_version"] == measurements["protocol_version"]',
    ):
        assert contract in run
