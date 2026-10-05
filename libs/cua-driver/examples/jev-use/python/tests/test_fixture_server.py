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


    def test_default_page_keeps_the_dom_submit_button(self) -> None:
        with self.request("/") as response:
            page = response.read().decode()
        self.assertIn('<button type="submit">Submit</button>', page)


class VisualFixtureServerTest(unittest.TestCase):
    def test_visual_page_has_no_submit_button_and_still_submits(self) -> None:
        server = FixtureServer(("127.0.0.1", 0), visual=True)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            url = f"http://127.0.0.1:{server.server_port}"
            with urlopen(f"{url}/", timeout=2) as response:
                page = response.read().decode()
            self.assertNotIn("<button", page)
            self.assertIn('role="presentation"', page)
            self.assertIn("requestSubmit()", page)
            self.assertIn('aria-label="verification value"', page)
            data = urlencode({"value": "proof"}).encode()
            with urlopen(Request(f"{url}/submit", method="POST", data=data), timeout=2):
                pass
            with urlopen(f"{url}/state", timeout=2) as response:
                self.assertEqual(json.loads(response.read()), {"submitted": "proof"})
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
