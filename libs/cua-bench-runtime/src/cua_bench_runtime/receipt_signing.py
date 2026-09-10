"""Cryptographic signing for driver-participation receipts."""

from __future__ import annotations

import base64
import binascii
import os
import re
import subprocess
import tempfile
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from cua_bench_runtime.canon import canonical_json, sha256_bytes
from cua_bench_runtime.errors import HarnessFailure, ValidationFailure
from cua_bench_runtime.process import clean_environment

NAMESPACE = "cua-bench-driver-participation-v1"
SIGNER_IDENTITY = "cua-bench-participation-receipt"
CERTIFICATION_NAMESPACE = "cua-bench-apparatus-certification-v1"
CERTIFICATION_SIGNER_IDENTITY = "cua-bench-apparatus-certification-receipt"
REPORT_PREREGISTRATION_NAMESPACE = "cua-bench-report-preregistration-v1"
REPORT_PREREGISTRATION_SIGNER_IDENTITY = "cua-bench-report-preregistration"
MAX_BODY_BYTES = 1_048_576
MAX_KEY_BYTES = 16_384
MAX_SIGNATURE_BYTES = 16_384

_DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
_KEY_ID = re.compile(r"^sha256:[0-9a-f]{64}$")
_NAMESPACE = re.compile(r"^[A-Za-z0-9._-]{1,64}$")
_BINDING_FIELDS = frozenset({"trial_id", "task_digest", "config_digest"})
_REPORT_BINDING_FIELDS = frozenset({"preregistration_digest"})
_BODY_FIELDS = frozenset({"namespace", "key_id", "bindings", "receipt"})
_ARTIFACT_FIELDS = frozenset({"body", "signature"})
_SSH_KEYGEN_CANDIDATES = (
    Path("/usr/bin/ssh-keygen"),
    Path(r"C:\Windows\System32\OpenSSH\ssh-keygen.exe"),
)
_SSH_KEYGEN = next(
    (str(candidate) for candidate in _SSH_KEYGEN_CANDIDATES if candidate.is_file()),
    None,
)


def _run(command: list[str], *, stdin: bytes | None = None) -> subprocess.CompletedProcess:
    if _SSH_KEYGEN is None:
        raise HarnessFailure("pinned system ssh-keygen is unavailable")
    extra_environment = {"LANG": "C", "LC_ALL": "C"}
    if os.name == "nt":
        extra_environment = {
            key: os.environ[key]
            for key in (
                "ALLUSERSPROFILE",
                "HOMEDRIVE",
                "HOMEPATH",
                "LOCALAPPDATA",
                "PROGRAMDATA",
                "USERNAME",
                "USERDOMAIN",
            )
            if key in os.environ
        }
    try:
        return subprocess.run(
            command,
            input=stdin,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=10,
            env=clean_environment(extra_environment),
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise HarnessFailure("could not execute ssh-keygen for receipt signing") from error


def _public_key_text(path: Path) -> str:
    try:
        if path.stat().st_size > MAX_KEY_BYTES:
            raise ValidationFailure("trusted Ed25519 public key is too large")
        text = path.read_text(encoding="ascii").strip()
    except (OSError, UnicodeError) as error:
        raise ValidationFailure("cannot read trusted Ed25519 public key") from error
    if len(text.splitlines()) != 1:
        raise ValidationFailure("trusted public key must contain one Ed25519 key")
    fields = text.split()
    if len(fields) < 2 or fields[0] != "ssh-ed25519":
        raise ValidationFailure("trusted public key must be OpenSSH Ed25519")
    try:
        wire = base64.b64decode(fields[1], validate=True)
    except (ValueError, binascii.Error) as error:
        raise ValidationFailure("trusted Ed25519 public key is malformed") from error
    algorithm = b"ssh-ed25519"
    expected_prefix = len(algorithm).to_bytes(4, "big") + algorithm
    if (
        not wire.startswith(expected_prefix)
        or len(wire) != len(expected_prefix) + 4 + 32
        or wire[len(expected_prefix) : len(expected_prefix) + 4] != (32).to_bytes(4, "big")
    ):
        raise ValidationFailure("trusted Ed25519 public key is malformed")
    return f"ssh-ed25519 {fields[1]}"


def _key_id_text(public_key: str) -> str:
    encoded = public_key.split()[1]
    return sha256_bytes(base64.b64decode(encoded))


def key_id(public_key: Path) -> str:
    """Return the stable SHA-256 identifier for an OpenSSH Ed25519 key."""

    return _key_id_text(_public_key_text(public_key))


def _bindings(value: Mapping[str, Any]) -> dict[str, str]:
    if set(value) != _BINDING_FIELDS:
        raise ValidationFailure(
            "receipt bindings must contain trial_id, task_digest, and config_digest"
        )
    trial_id = value.get("trial_id")
    if not isinstance(trial_id, str) or not trial_id:
        raise ValidationFailure("receipt binding trial_id must be non-empty")
    normalized = {"trial_id": trial_id}
    for field in ("task_digest", "config_digest"):
        digest = value.get(field)
        if not isinstance(digest, str) or not _DIGEST.fullmatch(digest):
            raise ValidationFailure(f"receipt binding {field} must be sha256:<hex>")
        normalized[field] = digest
    return normalized


def _normalized_bindings(value: Mapping[str, Any], namespace: str) -> dict[str, Any]:
    if namespace == CERTIFICATION_NAMESPACE:
        from cua_bench_runtime.certification import normalize_bindings

        return normalize_bindings(value)
    if namespace == REPORT_PREREGISTRATION_NAMESPACE:
        if set(value) != _REPORT_BINDING_FIELDS:
            raise ValidationFailure(
                "report preregistration bindings must contain preregistration_digest"
            )
        digest = value.get("preregistration_digest")
        if not isinstance(digest, str) or not _DIGEST.fullmatch(digest):
            raise ValidationFailure("report preregistration binding must be sha256:<hex>")
        return {"preregistration_digest": digest}
    return _bindings(value)


def _signer_identity(namespace: str) -> str:
    if namespace == CERTIFICATION_NAMESPACE:
        return CERTIFICATION_SIGNER_IDENTITY
    if namespace == REPORT_PREREGISTRATION_NAMESPACE:
        return REPORT_PREREGISTRATION_SIGNER_IDENTITY
    return SIGNER_IDENTITY


def _namespace(value: str) -> str:
    if not isinstance(value, str) or not _NAMESPACE.fullmatch(value):
        raise ValidationFailure("receipt namespace is invalid")
    return value


def _canonical_body(body: Mapping[str, Any]) -> bytes:
    try:
        encoded = canonical_json(dict(body))
    except (TypeError, ValueError) as error:
        raise ValidationFailure("signed receipt body is not canonical JSON") from error
    if len(encoded) > MAX_BODY_BYTES:
        raise ValidationFailure("signed receipt body exceeds the size limit")
    return encoded


def _derived_public_key(private_key: Path, destination: Path) -> str:
    result = _run([_SSH_KEYGEN, "-y", "-f", str(private_key)])
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).decode("utf-8", errors="replace").strip()
        detail = detail.replace(str(private_key), "<signing-key>")[-500:]
        suffix = f" (exit {result.returncode})"
        if detail:
            suffix += f": {detail}"
        raise HarnessFailure(f"ssh-keygen could not read the Ed25519 signing key{suffix}")
    try:
        destination.write_bytes(result.stdout)
    except OSError as error:
        raise HarnessFailure("could not stage the receipt public key") from error
    return _public_key_text(destination)


def sign_receipt(
    receipt: Mapping[str, Any],
    *,
    bindings: Mapping[str, Any],
    private_key: Path,
    namespace: str = NAMESPACE,
) -> dict[str, Any]:
    """Sign a canonical receipt body with an OpenSSH Ed25519 private key."""

    if not isinstance(receipt, Mapping):
        raise ValidationFailure("receipt must be an object")
    normalized_namespace = _namespace(namespace)
    normalized_bindings = _normalized_bindings(bindings, normalized_namespace)
    private_key = private_key.resolve()
    if not private_key.is_file():
        raise ValidationFailure("receipt signing key is not a file")
    if private_key.stat().st_size > MAX_KEY_BYTES:
        raise ValidationFailure("receipt signing key is too large")

    with tempfile.TemporaryDirectory(prefix="cb-receipt-sign-") as directory:
        root = Path(directory)
        public_key_path = root / "signer.pub"
        public_key = _derived_public_key(private_key, public_key_path)
        if not public_key.startswith("ssh-ed25519 "):
            raise ValidationFailure("receipt signing key must be Ed25519")
        body = {
            "namespace": normalized_namespace,
            "key_id": key_id(public_key_path),
            "bindings": normalized_bindings,
            "receipt": dict(receipt),
        }
        body_path = root / "body.json"
        body_path.write_bytes(_canonical_body(body))
        result = _run(
            [
                _SSH_KEYGEN,
                "-Y",
                "sign",
                "-f",
                str(private_key),
                "-n",
                normalized_namespace,
                str(body_path),
            ]
        )
        signature_path = body_path.with_suffix(".json.sig")
        if result.returncode != 0 or not signature_path.is_file():
            raise HarnessFailure("ssh-keygen could not sign the participation receipt")
        try:
            signature = signature_path.read_text(encoding="ascii")
        except (OSError, UnicodeError) as error:
            raise HarnessFailure("could not read the participation signature") from error
        if len(signature.encode("ascii")) > MAX_SIGNATURE_BYTES:
            raise HarnessFailure("participation signature exceeds the size limit")
    return {"body": body, "signature": signature}


def verify_receipt(
    artifact: Mapping[str, Any],
    *,
    trusted_public_key: Path,
    expected_bindings: Mapping[str, Any],
    namespace: str = NAMESPACE,
) -> dict[str, Any]:
    """Verify a receipt with one pinned key and return its signed payload."""

    if not isinstance(artifact, Mapping) or set(artifact) != _ARTIFACT_FIELDS:
        raise ValidationFailure("signed receipt artifact has unsupported fields")
    body = artifact.get("body")
    signature = artifact.get("signature")
    if not isinstance(body, Mapping) or set(body) != _BODY_FIELDS:
        raise ValidationFailure("signed receipt body has unsupported fields")
    if not isinstance(signature, str):
        raise ValidationFailure("signed receipt has no SSHSIG signature")
    try:
        signature_bytes = signature.encode("ascii")
    except UnicodeEncodeError as error:
        raise ValidationFailure("signed receipt has an invalid SSHSIG signature") from error
    if not signature.startswith("-----BEGIN SSH SIGNATURE-----\n") or not (
        signature.endswith("-----END SSH SIGNATURE-----\n")
    ):
        raise ValidationFailure("signed receipt has no SSHSIG signature")
    if len(signature_bytes) > MAX_SIGNATURE_BYTES:
        raise ValidationFailure("participation signature exceeds the size limit")

    normalized_namespace = _namespace(namespace)
    if body.get("namespace") != normalized_namespace:
        raise ValidationFailure("signed receipt namespace mismatch")
    actual_bindings = body.get("bindings")
    if not isinstance(actual_bindings, Mapping):
        raise ValidationFailure("signed receipt bindings must be an object")
    if _normalized_bindings(actual_bindings, normalized_namespace) != _normalized_bindings(
        expected_bindings, normalized_namespace
    ):
        raise ValidationFailure("signed receipt bindings mismatch")
    receipt = body.get("receipt")
    if not isinstance(receipt, Mapping):
        raise ValidationFailure("signed receipt payload must be an object")

    trusted_public_key = trusted_public_key.resolve()
    trusted_key = _public_key_text(trusted_public_key)
    trusted_key_id = _key_id_text(trusted_key)
    if not isinstance(body.get("key_id"), str) or not _KEY_ID.fullmatch(body["key_id"]):
        raise ValidationFailure("signed receipt key ID is invalid")
    if body["key_id"] != trusted_key_id:
        raise ValidationFailure("signed receipt key ID does not match trusted key")

    with tempfile.TemporaryDirectory(prefix="cb-receipt-verify-") as directory:
        root = Path(directory)
        signature_path = root / "receipt.sig"
        allowed_signers = root / "allowed_signers"
        signature_path.write_text(signature, encoding="ascii")
        allowed_signers.write_text(
            f"{_signer_identity(normalized_namespace)} {trusted_key}\n",
            encoding="ascii",
        )
        result = _run(
            [
                _SSH_KEYGEN,
                "-Y",
                "verify",
                "-f",
                str(allowed_signers),
                "-I",
                _signer_identity(normalized_namespace),
                "-n",
                normalized_namespace,
                "-s",
                str(signature_path),
            ],
            stdin=_canonical_body(body),
        )
    if result.returncode != 0:
        raise ValidationFailure("participation receipt signature verification failed")
    return dict(receipt)


def sign_certification_receipt(
    receipt: Mapping[str, Any],
    *,
    bindings: Mapping[str, Any],
    private_key: Path,
) -> dict[str, Any]:
    """Sign a complete apparatus-certification receipt under its own namespace."""

    return sign_receipt(
        receipt,
        bindings=bindings,
        private_key=private_key,
        namespace=CERTIFICATION_NAMESPACE,
    )


def verify_certification_signature(
    artifact: Mapping[str, Any],
    *,
    trusted_public_key: Path,
    expected_bindings: Mapping[str, Any],
) -> dict[str, Any]:
    """Verify one final apparatus receipt against exact trial bindings."""

    return verify_receipt(
        artifact,
        trusted_public_key=trusted_public_key,
        expected_bindings=expected_bindings,
        namespace=CERTIFICATION_NAMESPACE,
    )


def sign_report_preregistration(
    document: Mapping[str, Any], *, private_key: Path
) -> dict[str, Any]:
    """Sign one frozen report plan under a dedicated SSHSIG namespace."""

    digest = document.get("digest") if isinstance(document, Mapping) else None
    if not isinstance(digest, str) or not _DIGEST.fullmatch(digest):
        raise ValidationFailure("report preregistration digest is invalid")
    return sign_receipt(
        document,
        bindings={"preregistration_digest": digest},
        private_key=private_key,
        namespace=REPORT_PREREGISTRATION_NAMESPACE,
    )


def verify_report_preregistration_signature(
    artifact: Mapping[str, Any],
    *,
    trusted_public_key: Path,
) -> dict[str, Any]:
    """Verify one frozen report plan and return its signed document."""

    if not isinstance(artifact, Mapping):
        raise ValidationFailure("signed report preregistration is invalid")
    body = artifact.get("body")
    receipt = body.get("receipt") if isinstance(body, Mapping) else None
    digest = receipt.get("digest") if isinstance(receipt, Mapping) else None
    if not isinstance(digest, str) or not _DIGEST.fullmatch(digest):
        raise ValidationFailure("signed report preregistration digest is invalid")
    return verify_receipt(
        artifact,
        trusted_public_key=trusted_public_key,
        expected_bindings={"preregistration_digest": digest},
        namespace=REPORT_PREREGISTRATION_NAMESPACE,
    )
