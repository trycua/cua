import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Thread
from urllib.request import urlopen

from http.server import ThreadingHTTPServer

from dev_server import BuildState, make_handler


class BuildStateTests(unittest.TestCase):
    def test_initial_failure_reports_that_no_render_exists(self) -> None:
        state = BuildState()

        state.fail("synthetic compiler failure")

        status = state.snapshot()
        self.assertEqual(status["state"], "error")
        self.assertIsNone(status["build"])
        self.assertIn("no valid render", status["message"])

    def test_failed_rebuild_retains_last_valid_build(self) -> None:
        state = BuildState()
        first = {"id": "build-1", "manifest": {"theme_name": "Test"}}

        state.succeed(first)
        state.begin()
        state.fail("synthetic compiler failure")

        status = state.snapshot()
        self.assertEqual(status["state"], "error")
        self.assertEqual(status["build"], first)
        self.assertIsNone(status["previous_build"])
        self.assertIn("last valid render", status["message"])
        self.assertEqual(status["error"], "synthetic compiler failure")

    def test_successful_rebuild_exposes_previous_build_for_comparison(self) -> None:
        state = BuildState()
        first = {"id": "build-1"}
        second = {"id": "build-2"}

        state.succeed(first)
        state.succeed(second)

        status = state.snapshot()
        self.assertEqual(status["state"], "ready")
        self.assertEqual(status["build"], second)
        self.assertEqual(status["previous_build"], first)


class HandlerTests(unittest.TestCase):
    def test_index_marks_repo_dev_mode(self) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            gallery = root / "gallery"
            builds = root / "builds"
            gallery.mkdir()
            builds.mkdir()
            (gallery / "index.html").write_text('<html lang="en"><body>Gallery</body></html>')
            server = ThreadingHTTPServer(
                ("127.0.0.1", 0),
                make_handler(gallery, builds, BuildState()),
            )
            thread = Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                with urlopen(f"http://127.0.0.1:{server.server_port}/") as response:
                    body = response.read().decode()
                self.assertIn('data-dev-server="true"', body)
                self.assertEqual(body.count('data-dev-server="true"'), 1)
            finally:
                server.shutdown()
                server.server_close()
                thread.join()


if __name__ == "__main__":
    unittest.main()
