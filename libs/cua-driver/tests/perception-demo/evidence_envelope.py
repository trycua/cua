#!/usr/bin/env python3
"""Encrypt or decrypt the fixed visual perception evidence bundle."""

from __future__ import annotations

import argparse
import base64
import binascii
import ctypes
import errno
import hashlib
import hmac
from io import BytesIO
import json
import os
from pathlib import Path
import shutil
import stat
import struct
import sys
import tempfile
import zipfile

from cryptography.exceptions import InvalidTag
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.hkdf import HKDF


MEMBER_NAMES = ("manifest.json", "recording.mp4")
MANIFEST_SCHEMA = "cua-visual-perception-demo-evidence/v3"
MAX_MANIFEST_SIZE = 1024 * 1024
MAX_RECORDING_SIZE = 100 * 1024 * 1024
MAX_ARCHIVE_SIZE = MAX_MANIFEST_SIZE + MAX_RECORDING_SIZE + 4096

MAGIC = b"CUAEVID\x00"
VERSION = 3
ALGORITHM_X25519_HKDF_SHA256_AES_256_GCM = 2
NONCE_SIZE = 12
TAG_SIZE = 16
X25519_KEY_SIZE = 32
HEADER = struct.Struct(">8sBBH32s32s12sQ")
# Header fields: magic, version, algorithm, flags, recipient fingerprint,
# ephemeral public key, nonce, ciphertext size.
HKDF_INFO = b"cua-visual-perception-evidence-envelope/v3\x00recipient-sha256\x00"


def _regular_file(path: Path, label: str, maximum: int) -> bytes:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError as error:
        raise ValueError(f"{label} is missing") from error
    if not stat.S_ISREG(mode) or path.is_symlink():
        raise ValueError(f"{label} must be a regular file, not a link")
    size = path.stat().st_size
    if not 0 < size <= maximum:
        raise ValueError(f"{label} must be nonempty and at most {maximum} bytes")
    data = path.read_bytes()
    if len(data) != size:
        raise ValueError(f"{label} changed while it was being read")
    return data


def _validate_manifest(manifest_bytes: bytes, recording_bytes: bytes) -> None:
    try:
        manifest = json.loads(manifest_bytes.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError("manifest.json must be valid UTF-8 JSON") from error
    if not isinstance(manifest, dict) or manifest.get("schema") != MANIFEST_SCHEMA:
        raise ValueError(f"manifest.json must use {MANIFEST_SCHEMA}")

    expected = [{
        "kind": "video",
        "path": "recording.mp4",
        "sha256": hashlib.sha256(recording_bytes).hexdigest(),
        "size_bytes": len(recording_bytes),
    }]
    if manifest.get("artifacts") != expected:
        raise ValueError("manifest.json does not bind the supplied recording.mp4")


def _load_bundle(directory: Path) -> dict[str, bytes]:
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("input directory must be a real directory")
    names = sorted(item.name for item in directory.iterdir())
    if names != sorted(MEMBER_NAMES):
        raise ValueError(f"input directory must contain exactly {list(MEMBER_NAMES)}")
    members = {
        "manifest.json": _regular_file(directory / "manifest.json", "manifest.json", MAX_MANIFEST_SIZE),
        "recording.mp4": _regular_file(directory / "recording.mp4", "recording.mp4", MAX_RECORDING_SIZE),
    }
    _validate_manifest(members["manifest.json"], members["recording.mp4"])
    return members


def _build_archive(members: dict[str, bytes]) -> bytes:
    output = BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED, allowZip64=False) as archive:
        for name in MEMBER_NAMES:
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_STORED
            info.create_system = 3
            info.create_version = 20
            info.extract_version = 20
            info.flag_bits = 0
            info.external_attr = (stat.S_IFREG | 0o600) << 16
            archive.writestr(info, members[name])
    value = output.getvalue()
    if len(value) > MAX_ARCHIVE_SIZE:
        raise ValueError("evidence archive exceeds the maximum size")
    return value


def _read_archive(value: bytes) -> dict[str, bytes]:
    if not value or len(value) > MAX_ARCHIVE_SIZE:
        raise ValueError("decrypted evidence archive has an invalid size")
    try:
        with zipfile.ZipFile(BytesIO(value), "r") as archive:
            infos = archive.infolist()
            if [item.filename for item in infos] != list(MEMBER_NAMES):
                raise ValueError(f"evidence archive must contain exactly {list(MEMBER_NAMES)}")
            limits = (MAX_MANIFEST_SIZE, MAX_RECORDING_SIZE)
            for info, limit in zip(infos, limits):
                if info.is_dir() or info.flag_bits & 0x1 or info.compress_type != zipfile.ZIP_STORED:
                    raise ValueError("evidence archive contains an unsupported member")
                if not 0 < info.file_size <= limit or info.compress_size != info.file_size:
                    raise ValueError(f"{info.filename} has an invalid archived size")
            members = {info.filename: archive.read(info) for info in infos}
    except (zipfile.BadZipFile, RuntimeError) as error:
        raise ValueError("decrypted evidence archive is invalid") from error

    _validate_manifest(members["manifest.json"], members["recording.mp4"])
    if _build_archive(members) != value:
        raise ValueError("evidence archive is not in the canonical format")
    return members


def _decode_key(value: str, label: str) -> bytes:
    try:
        encoded = value.encode("ascii")
        key = base64.b64decode(encoded, validate=True)
    except (UnicodeEncodeError, binascii.Error) as error:
        raise ValueError(f"{label} must be canonical base64") from error
    if len(key) != X25519_KEY_SIZE or base64.b64encode(key) != encoded:
        raise ValueError(f"{label} must be canonical base64 for exactly 32 bytes")
    return key


def load_key(*, environment_name: str | None, key_file: Path | None, label: str) -> bytes:
    if (environment_name is None) == (key_file is None):
        raise ValueError("choose exactly one key source")
    if environment_name is not None:
        if not environment_name or not environment_name.replace("_", "A").isalnum():
            raise ValueError(f"{label} environment variable name is invalid")
        value = os.environ.get(environment_name)
        if value is None:
            raise ValueError(f"{label} environment variable {environment_name} is not set")
        return _decode_key(value, label)

    assert key_file is not None
    raw = _regular_file(key_file, f"{label} file", 256)
    try:
        value = raw.decode("ascii").rstrip("\r\n")
    except UnicodeDecodeError as error:
        raise ValueError(f"{label} file must contain ASCII base64") from error
    return _decode_key(value, label)


def _require_x25519_key(key: bytes, label: str) -> None:
    if not isinstance(key, bytes) or len(key) != X25519_KEY_SIZE:
        raise ValueError(f"{label} must contain exactly 32 bytes")


def _public_key_bytes(public_key: X25519PublicKey) -> bytes:
    return public_key.public_bytes(
        encoding=serialization.Encoding.Raw,
        format=serialization.PublicFormat.Raw,
    )


def recipient_fingerprint(public_key: bytes) -> bytes:
    _require_x25519_key(public_key, "recipient public key")
    return hashlib.sha256(public_key).digest()


def _derive_key(
    private_key: X25519PrivateKey,
    public_key: X25519PublicKey,
    recipient_key_fingerprint: bytes,
) -> bytes:
    if not isinstance(recipient_key_fingerprint, bytes) or len(recipient_key_fingerprint) != 32:
        raise ValueError("recipient key fingerprint must contain exactly 32 bytes")
    try:
        shared_secret = private_key.exchange(public_key)
    except ValueError as error:
        raise ValueError("X25519 key agreement failed") from error
    return HKDF(
        algorithm=hashes.SHA256(),
        length=32,
        salt=None,
        info=HKDF_INFO + recipient_key_fingerprint,
    ).derive(shared_secret)


def encrypt_bundle(input_dir: Path, output: Path, recipient_public_key: bytes) -> str:
    _require_x25519_key(recipient_public_key, "recipient public key")
    members = _load_bundle(input_dir)
    archive = _build_archive(members)
    recipient = X25519PublicKey.from_public_bytes(recipient_public_key)
    ephemeral_private = X25519PrivateKey.generate()
    ephemeral_public = _public_key_bytes(ephemeral_private.public_key())
    fingerprint = recipient_fingerprint(recipient_public_key)
    key = _derive_key(ephemeral_private, recipient, fingerprint)
    nonce = os.urandom(NONCE_SIZE)
    ciphertext_size = len(archive) + TAG_SIZE
    header = HEADER.pack(
        MAGIC,
        VERSION,
        ALGORITHM_X25519_HKDF_SHA256_AES_256_GCM,
        0,
        fingerprint,
        ephemeral_public,
        nonce,
        ciphertext_size,
    )
    ciphertext = AESGCM(key).encrypt(nonce, archive, header)
    _atomic_write(output, header + ciphertext)
    return fingerprint.hex()


def decrypt_bundle(input_path: Path, output_dir: Path, recipient_private_key: bytes) -> None:
    _require_x25519_key(recipient_private_key, "recipient private key")
    envelope = _regular_file(input_path, "encrypted evidence", HEADER.size + MAX_ARCHIVE_SIZE + TAG_SIZE)
    if len(envelope) < HEADER.size + TAG_SIZE:
        raise ValueError("encrypted evidence envelope is truncated")
    header = envelope[:HEADER.size]
    (
        magic,
        version,
        algorithm,
        flags,
        expected_recipient_fingerprint,
        ephemeral_public,
        nonce,
        ciphertext_size,
    ) = HEADER.unpack(header)
    if (
        magic != MAGIC
        or version != VERSION
        or algorithm != ALGORITHM_X25519_HKDF_SHA256_AES_256_GCM
        or flags != 0
    ):
        raise ValueError("encrypted evidence envelope has an unsupported format")
    ciphertext = envelope[HEADER.size:]
    if ciphertext_size != len(ciphertext) or ciphertext_size > MAX_ARCHIVE_SIZE + TAG_SIZE:
        raise ValueError("encrypted evidence envelope has an invalid length")
    private_key = X25519PrivateKey.from_private_bytes(recipient_private_key)
    actual_recipient_fingerprint = recipient_fingerprint(_public_key_bytes(private_key.public_key()))
    if not hmac.compare_digest(actual_recipient_fingerprint, expected_recipient_fingerprint):
        raise ValueError("recipient private key does not match envelope recipient fingerprint")
    public_key = X25519PublicKey.from_public_bytes(ephemeral_public)
    key = _derive_key(private_key, public_key, expected_recipient_fingerprint)
    try:
        archive = AESGCM(key).decrypt(nonce, ciphertext, header)
    except InvalidTag as error:
        raise ValueError("encrypted evidence authentication failed") from error
    members = _read_archive(archive)
    _atomic_write_directory(output_dir, members)


def _atomic_write(path: Path, value: bytes) -> None:
    if path.exists() or path.is_symlink():
        raise ValueError("output file already exists")
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(prefix=f".{path.name}.", dir=path.parent, delete=False) as handle:
            temporary = Path(handle.name)
            os.chmod(temporary, 0o600)
            handle.write(value)
            handle.flush()
            os.fsync(handle.fileno())
        try:
            os.link(temporary, path)
        except FileExistsError as error:
            raise ValueError("output file already exists") from error
        temporary.unlink()
        temporary = None
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def _atomic_write_directory(path: Path, members: dict[str, bytes]) -> None:
    if path.exists() or path.is_symlink():
        raise ValueError("output directory already exists")
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix=f".{path.name}.", dir=path.parent))
    try:
        for name in MEMBER_NAMES:
            target = temporary / name
            target.write_bytes(members[name])
            os.chmod(target, 0o600)
        try:
            _rename_directory_no_replace(temporary, path)
        except FileExistsError as error:
            raise ValueError("output directory already exists") from error
    finally:
        if temporary.exists():
            shutil.rmtree(temporary)


def _rename_directory_no_replace(source: Path, destination: Path) -> None:
    if os.name == "nt":
        # Windows rename already fails when the destination exists.
        os.rename(source, destination)
        return

    if sys.platform == "darwin":
        libc = ctypes.CDLL(None, use_errno=True)
        rename_no_replace = libc.renamex_np
        rename_no_replace.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
        arguments = (os.fsencode(source), os.fsencode(destination), 0x00000004)  # RENAME_EXCL
    elif sys.platform.startswith("linux"):
        libc = ctypes.CDLL(None, use_errno=True)
        try:
            rename_no_replace = libc.renameat2
        except AttributeError as error:
            raise OSError(
                errno.ENOTSUP,
                "atomic no-replace directory publication is unsupported",
            ) from error
        rename_no_replace.argtypes = [
            ctypes.c_int,
            ctypes.c_char_p,
            ctypes.c_int,
            ctypes.c_char_p,
            ctypes.c_uint,
        ]
        arguments = (-100, os.fsencode(source), -100, os.fsencode(destination), 1)  # AT_FDCWD, RENAME_NOREPLACE
    else:
        raise OSError(errno.ENOTSUP, "atomic no-replace directory publication is unsupported")

    rename_no_replace.restype = ctypes.c_int
    result = rename_no_replace(*arguments)
    if result == 0:
        return

    error_number = ctypes.get_errno()
    if error_number in (errno.EEXIST, errno.ENOTEMPTY):
        raise FileExistsError(error_number, os.strerror(error_number), destination)
    raise OSError(error_number, os.strerror(error_number), destination)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    encrypt = subparsers.add_parser("encrypt")
    encrypt.add_argument("--input", type=Path, required=True)
    encrypt.add_argument("--output", type=Path, required=True)
    recipient = encrypt.add_mutually_exclusive_group(required=True)
    recipient.add_argument("--recipient-env", metavar="NAME")
    recipient.add_argument("--recipient-file", type=Path)

    decrypt = subparsers.add_parser("decrypt")
    decrypt.add_argument("--input", type=Path, required=True)
    decrypt.add_argument("--output", type=Path, required=True)
    private = decrypt.add_mutually_exclusive_group(required=True)
    private.add_argument("--private-key-env", metavar="NAME")
    private.add_argument("--private-key-file", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "encrypt":
            recipient_public_key = load_key(
                environment_name=args.recipient_env,
                key_file=args.recipient_file,
                label="recipient public key",
            )
            fingerprint = encrypt_bundle(args.input, args.output, recipient_public_key)
            print(f"recipient_public_key_sha256={fingerprint}")
        else:
            recipient_private_key = load_key(
                environment_name=args.private_key_env,
                key_file=args.private_key_file,
                label="recipient private key",
            )
            decrypt_bundle(args.input, args.output, recipient_private_key)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
