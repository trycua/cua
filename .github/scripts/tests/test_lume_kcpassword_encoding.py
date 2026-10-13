"""Exercise the setup script's encoder without setup, sudo, or a real password."""
import ast
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[3]
KEY = bytes.fromhex("7d895223d2bcddeaa3b91f")


def encode_from_setup_script(password):
    script = (ROOT / "libs/lume/scripts/setup-cua.sh").read_text()
    block = script.split("# Create kcpassword file", 1)[1]
    block = block.split("python3 << PYEOF\n", 1)[1].split("\nPYEOF", 1)[0]
    # Stop before the file-writing statement. Exercise only the actual encoder.
    block = block.split('with open("/tmp/kcpassword"', 1)[0]
    tree = ast.parse(block)
    assignment = tree.body[0]
    assert isinstance(assignment, ast.Assign)
    assert isinstance(assignment.targets[0], ast.Name)
    assert assignment.targets[0].id == "password"
    assignment.value = ast.Constant(value=password)
    namespace = {}
    exec(compile(ast.fix_missing_locations(tree), "setup-cua-encoder", "exec"), namespace)
    return bytes(namespace["result"])


class KCPasswordEncodingTests(unittest.TestCase):
    def assert_wire_contract(self, password):
        wire = encode_from_setup_script(password)
        plaintext = password.encode("utf-8")
        self.assertEqual(len(wire), 12 * (len(plaintext) // 12 + 1))
        decoded = bytes(value ^ KEY[i % len(KEY)] for i, value in enumerate(wire))
        self.assertEqual(decoded[:len(plaintext)], plaintext)
        self.assertTrue(decoded[len(plaintext):])
        self.assertEqual(set(decoded[len(plaintext):]), {0})

    def test_ascii_block_boundaries(self):
        for length in (0, 1, 11, 12, 13, 23, 24, 25):
            with self.subTest(length=length):
                self.assert_wire_contract("a" * length)

    def test_utf8_byte_boundaries(self):
        for password in ("café", "é" * 6, "U0001f642" * 3, "e\u0301" * 4, "a" * 11 + "é"):
            with self.subTest(password=password):
                self.assert_wire_contract(password)


if __name__ == "__main__":
    unittest.main()
