from __future__ import annotations

import copy
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.receipt_signing import (
    CERTIFICATION_NAMESPACE,
    MAX_BODY_BYTES,
    NAMESPACE,
    key_id,
    sign_certification_receipt,
    sign_receipt,
    verify_certification_signature,
    verify_receipt,
)


BINDINGS = {
    "trial_id": "trial-17",
    "task_digest": "sha256:" + "1" * 64,
    "config_digest": "sha256:" + "2" * 64,
}
RECEIPT = {
    "required": True,
    "status": "passed",
    "evidence_event_hashes": ["sha256:" + "3" * 64],
}
CERTIFICATION_BINDINGS = {
    "trial_id": "trial-17",
    "task_digest": "sha256:" + "1" * 64,
    "system_digest": "sha256:" + "2" * 64,
    "execution_policy_digest": "sha256:" + "3" * 64,
    "config_digest": "sha256:" + "4" * 64,
    "inputs_manifest_digest": "sha256:" + "5" * 64,
    "seed_provenance_digest": "sha256:" + "6" * 64,
    "agent_digest": "sha256:" + "7" * 64,
    "evaluator_digest": "sha256:" + "8" * 64,
}


class ReceiptSigningTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.key = self.generate_key("trusted")
        self.other_key = self.generate_key("other")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def generate_key(self, name: str) -> Path:
        path = self.root / name
        ssh_keygen = (
            r"C:\Windows\System32\OpenSSH\ssh-keygen.exe" if os.name == "nt" else "ssh-keygen"
        )
        subprocess.run(
            [ssh_keygen, "-q", "-t", "ed25519", "-N", "", "-f", str(path)],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        if os.name == "nt":
            subprocess.run(
                [
                    "icacls",
                    str(path),
                    "/inheritance:r",
                    "/grant:r",
                    f"{os.environ['USERNAME']}:F",
                ],
                check=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        return path

    def signed(self) -> dict:
        return sign_receipt(RECEIPT, bindings=BINDINGS, private_key=self.key)

    def test_round_trip_uses_explicit_namespace_key_id_and_bindings(self) -> None:
        artifact = self.signed()

        verified = verify_receipt(
            artifact,
            trusted_public_key=self.key.with_suffix(".pub"),
            expected_bindings=BINDINGS,
        )

        self.assertEqual(verified, RECEIPT)
        self.assertEqual(artifact["body"]["namespace"], NAMESPACE)
        self.assertEqual(artifact["body"]["key_id"], key_id(self.key.with_suffix(".pub")))
        self.assertEqual(artifact["body"]["bindings"], BINDINGS)
        self.assertTrue(artifact["signature"].startswith("-----BEGIN SSH SIGNATURE-----"))

    def test_body_key_order_does_not_affect_verification(self) -> None:
        artifact = self.signed()
        artifact["body"] = dict(reversed(tuple(artifact["body"].items())))

        verified = verify_receipt(
            artifact,
            trusted_public_key=self.key.with_suffix(".pub"),
            expected_bindings=BINDINGS,
        )

        self.assertEqual(verified, RECEIPT)

    def test_tampered_body_fails_closed(self) -> None:
        artifact = self.signed()
        artifact["body"]["receipt"]["status"] = "failed"

        with self.assertRaisesRegex(ValidationFailure, "signature verification"):
            verify_receipt(
                artifact,
                trusted_public_key=self.key.with_suffix(".pub"),
                expected_bindings=BINDINGS,
            )

    def test_malformed_signature_fails_closed(self) -> None:
        artifact = self.signed()
        artifact["signature"] += "trailing-data"

        with self.assertRaisesRegex(ValidationFailure, "SSHSIG signature"):
            verify_receipt(
                artifact,
                trusted_public_key=self.key.with_suffix(".pub"),
                expected_bindings=BINDINGS,
            )

    def test_wrong_pinned_key_fails_closed(self) -> None:
        with self.assertRaisesRegex(ValidationFailure, "key ID"):
            verify_receipt(
                self.signed(),
                trusted_public_key=self.other_key.with_suffix(".pub"),
                expected_bindings=BINDINGS,
            )

    def test_valid_signature_cannot_be_replayed_for_other_bindings(self) -> None:
        replayed_bindings = {
            **BINDINGS,
            "trial_id": "trial-18",
        }
        with self.assertRaisesRegex(ValidationFailure, "bindings mismatch"):
            verify_receipt(
                self.signed(),
                trusted_public_key=self.key.with_suffix(".pub"),
                expected_bindings=replayed_bindings,
            )

    def test_signed_metadata_tampering_fails_closed(self) -> None:
        artifact = self.signed()
        namespace_tamper = copy.deepcopy(artifact)
        namespace_tamper["body"]["namespace"] = "other-namespace"
        with self.assertRaisesRegex(ValidationFailure, "namespace mismatch"):
            verify_receipt(
                namespace_tamper,
                trusted_public_key=self.key.with_suffix(".pub"),
                expected_bindings=BINDINGS,
            )

        key_id_tamper = copy.deepcopy(artifact)
        key_id_tamper["body"]["key_id"] = "sha256:" + "f" * 64
        with self.assertRaisesRegex(ValidationFailure, "key ID"):
            verify_receipt(
                key_id_tamper,
                trusted_public_key=self.key.with_suffix(".pub"),
                expected_bindings=BINDINGS,
            )

    def test_oversized_receipt_is_rejected_before_signing(self) -> None:
        with self.assertRaisesRegex(ValidationFailure, "size limit"):
            sign_receipt(
                {"evidence": "x" * MAX_BODY_BYTES},
                bindings=BINDINGS,
                private_key=self.key,
            )

    def test_certification_namespace_roundtrip_and_replay_protection(self) -> None:
        receipt = {"schema_version": 1, "eligible": True}
        artifact = sign_certification_receipt(
            receipt,
            bindings=CERTIFICATION_BINDINGS,
            private_key=self.key,
        )
        verified = verify_certification_signature(
            artifact,
            trusted_public_key=self.key.with_suffix(".pub"),
            expected_bindings=CERTIFICATION_BINDINGS,
        )
        self.assertEqual(verified, receipt)
        self.assertEqual(artifact["body"]["namespace"], CERTIFICATION_NAMESPACE)
        replayed = {
            **CERTIFICATION_BINDINGS,
            "seed_provenance_digest": "sha256:" + "9" * 64,
        }
        with self.assertRaisesRegex(ValidationFailure, "bindings mismatch"):
            verify_certification_signature(
                artifact,
                trusted_public_key=self.key.with_suffix(".pub"),
                expected_bindings=replayed,
            )

    def test_participation_signature_cannot_be_used_as_certification(self) -> None:
        with self.assertRaisesRegex(ValidationFailure, "namespace mismatch"):
            verify_certification_signature(
                self.signed(),
                trusted_public_key=self.key.with_suffix(".pub"),
                expected_bindings=CERTIFICATION_BINDINGS,
            )


if __name__ == "__main__":
    unittest.main()
