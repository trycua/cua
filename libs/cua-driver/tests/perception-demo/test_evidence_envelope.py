import base64
from contextlib import redirect_stdout
import hashlib
import importlib.util
from io import BytesIO, StringIO
import json
import os
from pathlib import Path
import stat
import tempfile
import unittest
from unittest import mock
import zipfile


HERE = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location("evidence_envelope", HERE / "evidence_envelope.py")
envelope = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(envelope)


class EvidenceEnvelopeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        recording = b"small deterministic mp4 fixture"
        (self.bundle / "recording.mp4").write_bytes(recording)
        manifest = {
            "schema": envelope.MANIFEST_SCHEMA,
            "artifacts": [{
                "kind": "video",
                "path": "recording.mp4",
                "sha256": hashlib.sha256(recording).hexdigest(),
                "size_bytes": len(recording),
            }],
        }
        (self.bundle / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
        self.private_key = bytes(range(1, 33))
        self.public_key = (
            envelope.X25519PrivateKey.from_private_bytes(self.private_key)
            .public_key()
            .public_bytes(
                encoding=envelope.serialization.Encoding.Raw,
                format=envelope.serialization.PublicFormat.Raw,
            )
        )

    def tearDown(self):
        self.temporary.cleanup()

    def write_authenticated_archive(self, archive):
        ephemeral_private = envelope.X25519PrivateKey.from_private_bytes(b"e" * 32)
        ephemeral_public = envelope._public_key_bytes(ephemeral_private.public_key())
        fingerprint = envelope.recipient_fingerprint(self.public_key)
        nonce = b"n" * envelope.NONCE_SIZE
        size = len(archive) + envelope.TAG_SIZE
        header = envelope.HEADER.pack(
            envelope.MAGIC,
            envelope.VERSION,
            envelope.ALGORITHM_X25519_HKDF_SHA256_AES_256_GCM,
            0,
            fingerprint,
            ephemeral_public,
            nonce,
            size,
        )
        recipient = envelope.X25519PublicKey.from_public_bytes(self.public_key)
        key = envelope._derive_key(ephemeral_private, recipient, fingerprint)
        encrypted = self.root / "crafted.cuae"
        encrypted.write_bytes(header + envelope.AESGCM(key).encrypt(nonce, archive, header))
        return encrypted

    def test_round_trip_has_fixed_envelope_and_archive_contract(self):
        encrypted = self.root / "evidence.cuae"
        restored = self.root / "restored"
        envelope.encrypt_bundle(self.bundle, encrypted, self.public_key)

        self.assertEqual(encrypted.read_bytes()[:8], envelope.MAGIC)
        header = envelope.HEADER.unpack(encrypted.read_bytes()[:envelope.HEADER.size])
        self.assertEqual(header[1], 3)
        self.assertEqual(header[4], hashlib.sha256(self.public_key).digest())
        envelope.decrypt_bundle(encrypted, restored, self.private_key)

        self.assertEqual(sorted(item.name for item in restored.iterdir()), sorted(envelope.MEMBER_NAMES))
        for name in envelope.MEMBER_NAMES:
            self.assertEqual((restored / name).read_bytes(), (self.bundle / name).read_bytes())

    def test_archive_bytes_are_deterministic(self):
        members = envelope._load_bundle(self.bundle)
        self.assertEqual(envelope._build_archive(members), envelope._build_archive(members))

    def test_tampered_ciphertext_is_rejected_without_output(self):
        encrypted = self.root / "evidence.cuae"
        output = self.root / "restored"
        envelope.encrypt_bundle(self.bundle, encrypted, self.public_key)
        value = bytearray(encrypted.read_bytes())
        value[-1] ^= 1
        encrypted.write_bytes(value)

        with self.assertRaisesRegex(ValueError, "authentication failed"):
            envelope.decrypt_bundle(encrypted, output, self.private_key)
        self.assertFalse(output.exists())

    def test_wrong_recipient_is_rejected_before_decryption_without_output(self):
        encrypted = self.root / "evidence.cuae"
        output = self.root / "restored"
        envelope.encrypt_bundle(self.bundle, encrypted, self.public_key)

        with mock.patch.object(envelope.AESGCM, "decrypt") as decrypt:
            with self.assertRaisesRegex(
                ValueError, "private key does not match envelope recipient fingerprint"
            ):
                envelope.decrypt_bundle(encrypted, output, b"x" * 32)
            decrypt.assert_not_called()
        self.assertFalse(output.exists())

    def test_tampered_recipient_fingerprint_is_rejected_without_output(self):
        encrypted = self.root / "evidence.cuae"
        output = self.root / "restored"
        envelope.encrypt_bundle(self.bundle, encrypted, self.public_key)
        value = bytearray(encrypted.read_bytes())
        value[12] ^= 1
        encrypted.write_bytes(value)

        with self.assertRaisesRegex(
            ValueError, "private key does not match envelope recipient fingerprint"
        ):
            envelope.decrypt_bundle(encrypted, output, self.private_key)
        self.assertFalse(output.exists())

    def test_refuses_extra_input_file_and_existing_outputs(self):
        (self.bundle / "raw-manifest.json").write_text("private", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "exactly"):
            envelope.encrypt_bundle(self.bundle, self.root / "evidence.cuae", self.public_key)
        (self.bundle / "raw-manifest.json").unlink()

        encrypted = self.root / "evidence.cuae"
        encrypted.write_bytes(b"keep")
        with self.assertRaisesRegex(ValueError, "already exists"):
            envelope.encrypt_bundle(self.bundle, encrypted, self.public_key)
        self.assertEqual(encrypted.read_bytes(), b"keep")

        encrypted.unlink()
        envelope.encrypt_bundle(self.bundle, encrypted, self.public_key)
        restored = self.root / "restored"
        restored.mkdir()
        marker = restored / "keep.txt"
        marker.write_text("keep", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "already exists"):
            envelope.decrypt_bundle(encrypted, restored, self.private_key)
        self.assertEqual(marker.read_text(encoding="utf-8"), "keep")

    def test_encrypt_refuses_file_created_during_finalization(self):
        encrypted = self.root / "evidence.cuae"
        real_link = os.link

        def create_competing_file(source, destination):
            Path(destination).write_bytes(b"racer")
            return real_link(source, destination)

        with mock.patch.object(envelope.os, "link", side_effect=create_competing_file):
            with self.assertRaisesRegex(ValueError, "already exists"):
                envelope.encrypt_bundle(self.bundle, encrypted, self.public_key)

        self.assertEqual(encrypted.read_bytes(), b"racer")
        self.assertEqual(list(self.root.glob(f".{encrypted.name}.*")), [])

    def test_decrypt_refuses_directory_created_during_finalization(self):
        encrypted = self.root / "evidence.cuae"
        restored = self.root / "restored"
        envelope.encrypt_bundle(self.bundle, encrypted, self.public_key)
        real_rename = envelope._rename_directory_no_replace
        competing_identity = []

        def create_competing_directory(source, destination):
            destination = Path(destination)
            destination.mkdir()
            status = destination.stat()
            competing_identity.append((status.st_dev, status.st_ino))
            return real_rename(source, destination)

        with mock.patch.object(
            envelope,
            "_rename_directory_no_replace",
            side_effect=create_competing_directory,
        ):
            with self.assertRaisesRegex(ValueError, "already exists"):
                envelope.decrypt_bundle(encrypted, restored, self.private_key)

        status = restored.stat()
        self.assertEqual((status.st_dev, status.st_ino), competing_identity[0])
        self.assertEqual(list(restored.iterdir()), [])
        self.assertEqual(list(self.root.glob(f".{restored.name}.*")), [])

    def test_refuses_symlink_member(self):
        (self.bundle / "recording.mp4").unlink()
        target = self.root / "outside.mp4"
        target.write_bytes(b"outside")
        try:
            (self.bundle / "recording.mp4").symlink_to(target)
        except (NotImplementedError, OSError):
            self.skipTest("symlinks are unavailable on this host")
        with self.assertRaisesRegex(ValueError, "regular file"):
            envelope.encrypt_bundle(self.bundle, self.root / "evidence.cuae", self.public_key)

    def test_refuses_manifest_mismatch_and_invalid_x25519_key_length(self):
        manifest_path = self.bundle / "manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["artifacts"][0]["sha256"] = "0" * 64
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "does not bind"):
            envelope.encrypt_bundle(self.bundle, self.root / "evidence.cuae", self.public_key)

        with self.assertRaisesRegex(ValueError, "exactly 32 bytes"):
            envelope.encrypt_bundle(self.bundle, self.root / "evidence.cuae", b"short")

    def test_loads_only_canonical_32_byte_base64_from_env_or_file(self):
        encoded = base64.b64encode(self.public_key).decode("ascii")
        with mock.patch.dict(os.environ, {"CUA_EVIDENCE_RECIPIENT": encoded}, clear=True):
            self.assertEqual(
                envelope.load_key(
                    environment_name="CUA_EVIDENCE_RECIPIENT",
                    key_file=None,
                    label="recipient public key",
                ),
                self.public_key,
            )

        key_file = self.root / "key.txt"
        key_file.write_text(encoded + "\n", encoding="ascii")
        self.assertEqual(
            envelope.load_key(
                environment_name=None,
                key_file=key_file,
                label="recipient private key",
            ),
            self.public_key,
        )
        for invalid in ("not-base64", base64.b64encode(b"short").decode("ascii"), encoded.rstrip("=")):
            with self.assertRaisesRegex(ValueError, "base64"):
                envelope._decode_key(invalid, "recipient public key")

    def test_each_envelope_uses_a_distinct_ephemeral_x25519_key(self):
        first = self.root / "first.cuae"
        second = self.root / "second.cuae"
        envelope.encrypt_bundle(self.bundle, first, self.public_key)
        envelope.encrypt_bundle(self.bundle, second, self.public_key)

        first_header = envelope.HEADER.unpack(first.read_bytes()[:envelope.HEADER.size])
        second_header = envelope.HEADER.unpack(second.read_bytes()[:envelope.HEADER.size])
        self.assertEqual(first_header[4], envelope.recipient_fingerprint(self.public_key))
        self.assertEqual(second_header[4], envelope.recipient_fingerprint(self.public_key))
        self.assertNotEqual(first_header[5], second_header[5])

    def test_recipient_fingerprint_is_bound_into_hkdf(self):
        ephemeral_private = envelope.X25519PrivateKey.from_private_bytes(b"e" * 32)
        recipient = envelope.X25519PublicKey.from_public_bytes(self.public_key)
        fingerprint = envelope.recipient_fingerprint(self.public_key)

        expected = envelope._derive_key(ephemeral_private, recipient, fingerprint)
        different = envelope._derive_key(ephemeral_private, recipient, b"x" * 32)

        self.assertNotEqual(expected, different)

    def test_tampered_authenticated_header_is_rejected(self):
        encrypted = self.root / "evidence.cuae"
        output = self.root / "restored"
        envelope.encrypt_bundle(self.bundle, encrypted, self.public_key)
        value = bytearray(encrypted.read_bytes())
        value[envelope.HEADER.size - 9] ^= 1
        encrypted.write_bytes(value)

        with self.assertRaisesRegex(ValueError, "authentication failed"):
            envelope.decrypt_bundle(encrypted, output, self.private_key)
        self.assertFalse(output.exists())

    def test_cli_logs_only_the_safe_recipient_fingerprint(self):
        encoded = base64.b64encode(self.public_key).decode("ascii")
        encrypted = self.root / "evidence.cuae"
        stdout = StringIO()
        with mock.patch.dict(os.environ, {"RECIPIENT": encoded}, clear=True), mock.patch(
            "sys.argv",
            [
                "evidence_envelope.py",
                "encrypt",
                "--input",
                str(self.bundle),
                "--output",
                str(encrypted),
                "--recipient-env",
                "RECIPIENT",
            ],
        ), redirect_stdout(stdout):
            self.assertEqual(envelope.main(), 0)

        self.assertEqual(
            stdout.getvalue().strip(),
            f"recipient_public_key_sha256={hashlib.sha256(self.public_key).hexdigest()}",
        )
        self.assertNotIn(encoded, stdout.getvalue())

    def test_refuses_trailing_archive_data_even_when_authenticated(self):
        members = envelope._load_bundle(self.bundle)
        noncanonical = envelope._build_archive(members) + b"trailing"
        encrypted = self.write_authenticated_archive(noncanonical)

        with self.assertRaisesRegex(ValueError, "canonical format"):
            envelope.decrypt_bundle(encrypted, self.root / "restored", self.private_key)

    def test_refuses_noncanonical_member_order_even_when_authenticated(self):
        members = envelope._load_bundle(self.bundle)
        output = BytesIO()
        with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED, allowZip64=False) as archive:
            for name in reversed(envelope.MEMBER_NAMES):
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_STORED
                info.create_system = 3
                info.create_version = 20
                info.extract_version = 20
                info.flag_bits = 0
                info.external_attr = (stat.S_IFREG | 0o600) << 16
                archive.writestr(info, members[name])
        encrypted = self.write_authenticated_archive(output.getvalue())

        with self.assertRaisesRegex(ValueError, "must contain exactly"):
            envelope.decrypt_bundle(encrypted, self.root / "restored", self.private_key)


if __name__ == "__main__":
    unittest.main()
