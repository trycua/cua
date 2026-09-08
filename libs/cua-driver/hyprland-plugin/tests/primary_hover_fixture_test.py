"""Source contract/preflight checks only; Linux compilation and native proof remain required."""
import hashlib
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import production_active_primary_proof as proof


class SourceContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = Path(__file__).with_name('primary_hover_fixture.c').read_text()

    def test_peer_identity_precedes_protocol_binding_or_input(self):
        source = self.source
        peer = source.index('getsockopt(')
        self.assertLess(peer, source.index('wl_display_get_registry(display)'))
        self.assertIn('peer.pid != expected || peer.uid != getuid()', source)
        self.assertIn('SO_PEERCRED', source)

    def test_ready_has_no_input_and_only_one_explicit_motion_is_possible(self):
        source = self.source
        self.assertNotIn('zwlr_virtual_pointer_v1_button(', source)
        self.assertEqual(source.count('zwlr_virtual_pointer_v1_motion_absolute('), 1)
        self.assertLess(source.index('event("ready",'), source.index('sscanf(command,'))
        self.assertLess(source.index('sscanf(command,'), source.index('zwlr_virtual_pointer_v1_motion_absolute('))
        motion = source.index('zwlr_virtual_pointer_v1_motion_absolute(')
        ack = source.index('event("moved",')
        self.assertIn('start_sync(display);', source[motion:ack])
        self.assertIn('if (!synced || stopped) return 1;', source[motion:ack])
        self.assertIn('failed = !closed;', source[ack:])

    def test_all_waits_are_bounded_and_partial_stdin_never_blocks_a_line_read(self):
        source = self.source
        self.assertNotIn('wl_display_roundtrip(', source)
        self.assertNotIn('fgets(', source)
        self.assertNotIn('scanf(', source.replace('sscanf(', ''))
        for line in source.splitlines():
            if line.strip().startswith('while ('):
                self.assertIn('now_ns() < deadline', line)
        self.assertIn('x >= width || y >= height', source)
        self.assertIn('return failed || !closed;', source)


class BinaryPreflightTests(unittest.TestCase):
    def test_alias_replaced_inode_wrong_hash_or_unreviewed_source_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            source = root / 'libs/cua-driver/hyprland-plugin/tests/primary_hover_fixture.c'
            source.parent.mkdir(parents=True)
            source.write_text('reviewed fixture source')
            binary = root / 'primary_hover_fixture'
            binary.write_bytes(b'reviewed executable fixture')
            binary.chmod(0o700)
            info = binary.stat()
            expected = {'path': str(binary), 'device': info.st_dev, 'inode': info.st_ino,
                'uid': info.st_uid, 'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                'source_sha256': hashlib.sha256(source.read_bytes()).hexdigest()}
            fixture = object.__new__(proof.HoverFixture)
            fixture.args = SimpleNamespace(source=root, hover_fixture=binary)
            fixture.expected = expected
            fixture.check_binary()
            for field, value in (('path', str(root / 'other')), ('device', info.st_dev + 1),
                                 ('inode', info.st_ino + 1), ('uid', info.st_uid + 1),
                                 ('sha256', '0' * 64), ('source_sha256', '0' * 64)):
                fixture.expected = {**expected, field: value}
                with self.subTest(field=field), self.assertRaises(AssertionError):
                    fixture.check_binary()
            fixture.expected = expected
            alias = root / 'alias'
            alias.symlink_to(binary)
            fixture.args.hover_fixture = alias
            fixture.expected = {**expected, 'path': str(alias)}
            with self.assertRaises(AssertionError): fixture.check_binary()
            fixture.args.hover_fixture = binary
            fixture.expected = expected
            with patch.object(os, 'access', return_value=False), self.assertRaises(AssertionError):
                fixture.check_binary()


if __name__ == '__main__':
    unittest.main()
