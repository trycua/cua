"""Host-only signing tests. Every private key here is a disposable test key."""

import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import input_operator as operator


EPOCH, CHALLENGE, TARGET = "a" * 32, "b" * 32, "c" * 32


class CanonicalTest(unittest.TestCase):
    def test_exact_bytes_and_final_newline(self):
        self.assertEqual(
            operator.canonical_message(EPOCH, CHALLENGE, TARGET, "1700000000123", "15"),
            b"CUA_TEST_LEASE_1\n" + b"a" * 32 + b"\n" + b"b" * 32
            + b"\n" + b"c" * 32 + b"\n1700000000123\n15\n",
        )

    def test_malformed_hex(self):
        for value in ("a" * 31, "a" * 33, "A" * 32, "g" * 32, "0x" + EPOCH, EPOCH + "\n", None, 1):
            for index in range(3):
                tokens = [EPOCH, CHALLENGE, TARGET]
                tokens[index] = value
                with self.subTest(value=value, index=index), self.assertRaises(ValueError):
                    operator.canonical_message(*tokens, 1000, 1)

    def test_malformed_numbers(self):
        for value in (-1, True, 1.0, "01", "+1", " 1", "1\n", "1.0", "nan", "inf", "1e3", str(1 << 64), "9" * 100):
            with self.subTest(value=value), self.assertRaises(ValueError):
                operator.canonical_message(EPOCH, CHALLENGE, TARGET, value, 1)
        for value in (0, 16, -1, True, "01", "1.0"):
            with self.subTest(caps=value), self.assertRaises(ValueError):
                operator.canonical_message(EPOCH, CHALLENGE, TARGET, 1000, value)

    def test_expiry_is_future_and_at_most_sixty_seconds(self):
        self.assertEqual(operator.validate_expiry(60_001, 1), 60_001)
        self.assertEqual(operator.validate_expiry(2, 1), 2)
        for expiry in (0, 1, 60_002):
            with self.subTest(expiry=expiry), self.assertRaises(ValueError):
                operator.validate_expiry(expiry, 1)

    def test_invalid_arguments_do_not_open_key_or_echo_content(self):
        out, err = io.StringIO(), io.StringIO()
        with patch.object(operator, "load_key") as load, contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            result = operator.main(["sign", "--key", "/private/sensitive-path", "--epoch", "sensitive-invalid-value",
                                    "--challenge", CHALLENGE, "--target", TARGET, "--capabilities", "1"])
        self.assertEqual(result, 1)
        load.assert_not_called()
        self.assertEqual(out.getvalue(), "")
        self.assertNotIn("sensitive", err.getvalue())


try:
    operator.crypto()
    HAS_CRYPTO = True
except RuntimeError:
    HAS_CRYPTO = False


@unittest.skipUnless(HAS_CRYPTO, "optional cryptography package is unavailable")
class KeyTest(unittest.TestCase):
    def test_exclusive_owner_only_key_and_public_output(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "test.pem"
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                self.assertEqual(operator.main(["keygen", "--key", str(path)]), 0)
            output = json.loads(out.getvalue())
            self.assertEqual(set(output), {"public_key"})
            self.assertRegex(output["public_key"], r"^[0-9a-f]{64}$")
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            original = path.read_bytes()
            with self.assertRaises(FileExistsError):
                operator.keygen(path)
            self.assertEqual(path.read_bytes(), original)
            self.assertEqual(operator.public_key_hex(operator.load_key(path)), output["public_key"])
            self.assertNotIn("PRIVATE", out.getvalue())

    def test_signed_packet_verifies_and_is_bound_to_fields(self):
        _, key_type = operator.crypto()
        key = key_type.generate()
        grant = operator.sign_grant(key, EPOCH, CHALLENGE, TARGET, 60_001, 15, 1)
        packet = grant["packet"].split(" ")
        self.assertEqual(packet[:5], ["APPROVE", CHALLENGE, TARGET, "60001", "15"])
        signature = bytes.fromhex(packet[5])
        self.assertEqual(len(signature), 64)
        key.public_key().verify(signature, operator.canonical_message(EPOCH, CHALLENGE, TARGET, 60_001, 15))
        from cryptography.exceptions import InvalidSignature
        for tokens in (("d" * 32, CHALLENGE, TARGET, 60_001, 15),
                       (EPOCH, "d" * 32, TARGET, 60_001, 15),
                       (EPOCH, CHALLENGE, "d" * 32, 60_001, 15),
                       (EPOCH, CHALLENGE, TARGET, 60_000, 15),
                       (EPOCH, CHALLENGE, TARGET, 60_001, 1)):
            with self.subTest(tokens=tokens), self.assertRaises(InvalidSignature):
                key.public_key().verify(signature, operator.canonical_message(*tokens))
        self.assertNotIn("private", json.dumps(grant).lower())

    def test_expired_or_overlong_grants_are_never_signed(self):
        from unittest.mock import Mock
        key = Mock()
        for expiry in (1000, 61_001):
            with self.assertRaises(ValueError):
                operator.sign_grant(key, EPOCH, CHALLENGE, TARGET, expiry, 1, 1000)
        key.sign.assert_not_called()

    def test_key_permissions_symlinks_and_parse_errors(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "test.pem"
            operator.keygen(path)
            os.chmod(path, 0o644)
            with self.assertRaises(ValueError):
                operator.load_key(path)
            os.chmod(path, 0o600)
            link = Path(root) / "link.pem"
            link.symlink_to(path)
            with self.assertRaises(OSError):
                operator.load_key(link)
            with patch.object(operator.os, "getuid", return_value=-1):
                with self.assertRaises(ValueError):
                    operator.load_key(path)

    def test_sign_cli_only_emits_public_grant(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "test.pem"
            operator.keygen(path)
            out = io.StringIO()
            with contextlib.redirect_stdout(out), patch.object(operator.time, "time_ns", return_value=1_000_000_000):
                self.assertEqual(operator.main(["sign", "--key", str(path), "--epoch", EPOCH,
                                               "--challenge", CHALLENGE, "--target", TARGET,
                                               "--capabilities", "1", "--ttl-ms", "60000"]), 0)
            grant = json.loads(out.getvalue())
            self.assertEqual(grant["expires_unix_ms"], 61_000)
            self.assertNotIn(str(path), out.getvalue())
            self.assertNotIn("PRIVATE", out.getvalue())


if __name__ == "__main__":
    unittest.main()
