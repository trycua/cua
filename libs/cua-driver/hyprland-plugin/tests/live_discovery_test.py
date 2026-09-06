"""Protocol helper checks; native evidence still requires a live compositor."""

import os
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import Mock

import live_discovery as discovery


class DiscoveryTest(unittest.TestCase):
    def test_exchange_sends_and_correlates_request(self):
        packet = discovery.HEADER.pack(b"CUA2", 2, 0, 3, 0, 17, 0)
        client = Mock()
        client.send.return_value = len(packet)
        client.recv.return_value = discovery.HEADER.pack(b"CUA2", 2, 0, 4, 0, 17, 0)
        self.assertEqual(discovery.exchange(client, 3, 17), (4, b""))
        client.send.assert_called_once_with(packet)

    def test_exchange_rejects_short_send_before_receive(self):
        client = Mock()
        client.send.return_value = 0
        with self.assertRaises(AssertionError):
            discovery.exchange(client, 3, 17)
        client.recv.assert_not_called()

    def test_exchange_rejects_wrong_request_id(self):
        client = Mock()
        client.send.return_value = discovery.HEADER.size
        client.recv.return_value = discovery.HEADER.pack(b"CUA2", 2, 0, 4, 0, 18, 0)
        with self.assertRaises(AssertionError):
            discovery.exchange(client, 3, 17)

    def test_optimized_python_refuses_before_cli_or_compositor_access(self):
        script = str(Path(discovery.__file__).resolve())
        for options, optimize in ((["-O"], ""), ([], "1"), (["-OO"], "")):
            with self.subTest(options=options, optimize=optimize):
                result = subprocess.run(
                    [sys.executable, *options, script, "--help"],
                    env={**os.environ, "PYTHONOPTIMIZE": optimize},
                    capture_output=True,
                    text=True,
                    timeout=10,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Discovery validation requires assertions", result.stderr)
                self.assertNotIn("usage:", result.stdout)


if __name__ == "__main__":
    unittest.main()
