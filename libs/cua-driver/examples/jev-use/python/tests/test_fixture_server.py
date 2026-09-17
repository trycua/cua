from __future__ import annotations

import json
import sys
import threading
import unittest
from pathlib import Path
from urllib.parse import urlencode
from urllib.request import Request, urlopen

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from fixture_server import FixtureServer


class FixtureServerTest(unittest.TestCase):
    def setUp(self) -> None:
        self.server = FixtureServer(("127.0.0.1", 0))
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = f"http://127.0.0.1:{self.server.server_port}"

    def tearDown(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)

    def request(self, path: str, *, method: str = "GET", data: bytes | None = None):
        return urlopen(Request(f"{self.url}{path}", method=method, data=data), timeout=2)

    def test_submit_state_and_reset(self) -> None:
        with self.request("/submit", method="POST", data=urlencode({"value": "proof"}).encode()):
            pass
        with self.request("/state") as response:
            self.assertEqual(json.loads(response.read()), {"submitted": "proof"})
        with self.request("/reset", method="POST", data=b"") as response:
            self.assertEqual(response.status, 204)
        with self.request("/state") as response:
            self.assertEqual(json.loads(response.read()), {"submitted": None})


if __name__ == "__main__":
    unittest.main()
