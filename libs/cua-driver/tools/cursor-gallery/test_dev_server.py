import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Thread
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from http.server import ThreadingHTTPServer

from dev_server import (
    BuildState,
    Builder,
    MOTION_OVERRIDE_FIELDS,
    make_handler,
    sanitize_motion_override,
    serialize_motion_override,
)


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
    def make_server(self, root: Path):
        gallery = root / "gallery"
        builds = root / "builds"
        gallery.mkdir()
        builds.mkdir()
        (gallery / "index.html").write_text('<html lang="en"><body>Gallery</body></html>')
        state = BuildState()
        builder = Builder(root, state)
        server = ThreadingHTTPServer(
            ("127.0.0.1", 0),
            make_handler(gallery, builds, state, builder),
        )
        return server, builder

    def test_index_marks_repo_dev_mode(self) -> None:
        with TemporaryDirectory() as temporary:
            server, _builder = self.make_server(Path(temporary))
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

    def test_motion_override_post_round_trips_to_the_fixed_path(self) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            server, builder = self.make_server(root)
            thread = Thread(target=server.serve_forever, daemon=True)
            thread.start()
            base = f"http://127.0.0.1:{server.server_port}"
            try:
                request = Request(
                    f"{base}/__cursor_dev/motion-override",
                    data=b'{"peak_speed": 450.0, "turn_radius": 120.0}',
                    headers={"Content-Type": "application/json"},
                    method="POST",
                )
                with urlopen(request) as response:
                    self.assertEqual(response.status, 200)
                    payload = json.loads(response.read().decode())
                self.assertTrue(payload["ok"])
                self.assertEqual(payload["override"]["peak_speed"], 450.0)

                written = builder.motion_override_path.read_text()
                self.assertIn('"schema": "cua.cursor-gallery-motion-override/1"', written)
                self.assertEqual(json.loads(written)["turn_radius"], 120.0)
                # The builder now reports the override for the exporter command.
                self.assertIn(
                    "--motion-overrides",
                    " ".join(builder.exporter_command(Path("f"), Path("a"))),
                )

                # Reset with `null` removes the file.
                reset = Request(
                    f"{base}/__cursor_dev/motion-override",
                    data=b"null",
                    headers={"Content-Type": "application/json"},
                    method="POST",
                )
                with urlopen(reset) as response:
                    self.assertEqual(response.status, 200)
                self.assertFalse(builder.motion_override_path.exists())
            finally:
                server.shutdown()
                server.server_close()
                thread.join()

    def test_motion_override_post_rejects_hostile_bodies(self) -> None:
        with TemporaryDirectory() as temporary:
            server, _builder = self.make_server(Path(temporary))
            thread = Thread(target=server.serve_forever, daemon=True)
            thread.start()
            base = f"http://127.0.0.1:{server.server_port}"
            try:
                for hostile in (
                    b'{"idle_hide_ms": 5}',
                    b'{"peak_speed": "fast"}',
                    b'{"peak_speed": [900]}',
                    b'"just a string"',
                    b'{"schema": "other/1"}',
                ):
                    request = Request(
                        f"{base}/__cursor_dev/motion-override",
                        data=hostile,
                        headers={"Content-Type": "application/json"},
                        method="POST",
                    )
                    with self.assertRaises(HTTPError) as caught:
                        urlopen(request)
                    self.assertEqual(caught.exception.code, 400)
                # Unknown POST paths are simply not found.
                request = Request(
                    f"{base}/__cursor_dev/other",
                    data=b"{}",
                    headers={"Content-Type": "application/json"},
                    method="POST",
                )
                with self.assertRaises(HTTPError) as caught:
                    urlopen(request)
                self.assertEqual(caught.exception.code, 404)
            finally:
                server.shutdown()
                server.server_close()
                thread.join()


    def test_motion_override_post_rejects_oversized_and_cross_origin(self) -> None:
        with TemporaryDirectory() as temporary:
            server, _builder = self.make_server(Path(temporary))
            thread = Thread(target=server.serve_forever, daemon=True)
            thread.start()
            base = f"http://127.0.0.1:{server.server_port}"
            try:
                # Declared length over the cap is refused before the body is read.
                request = Request(
                    f"{base}/__cursor_dev/motion-override",
                    data=b'{"peak_speed": 900.0}',
                    headers={"Content-Length": str(1024 * 1024)},
                    method="POST",
                )
                with self.assertRaises(HTTPError) as caught:
                    urlopen(request)
                self.assertEqual(caught.exception.code, 413)

                # A foreign Origin (localhost CSRF from another page) is rejected.
                csrf = Request(
                    f"{base}/__cursor_dev/motion-override",
                    data=b'{"peak_speed": 900.0}',
                    headers={"Origin": "http://evil.example"},
                    method="POST",
                )
                with self.assertRaises(HTTPError) as caught:
                    urlopen(csrf)
                self.assertEqual(caught.exception.code, 403)

                # Same-origin Origin headers are accepted.
                ok = Request(
                    f"{base}/__cursor_dev/motion-override",
                    data=b"null",
                    headers={"Origin": base},
                    method="POST",
                )
                with urlopen(ok) as response:
                    self.assertEqual(response.status, 200)
            finally:
                server.shutdown()
                server.server_close()
                thread.join()


class MotionOverrideValidationTests(unittest.TestCase):
    def test_clamps_values_into_the_production_bounds(self) -> None:
        override = sanitize_motion_override(
            b'{"peak_speed": 99999.0, "min_end_speed": -5.0, "spring": 9.0}'
        )
        self.assertEqual(override["peak_speed"], 5000.0)
        self.assertEqual(override["min_end_speed"], 1.0)
        self.assertEqual(override["spring"], 1.0)

    def test_null_resets_and_unknown_fields_are_rejected(self) -> None:
        self.assertEqual(sanitize_motion_override(b"null"), {})
        with self.assertRaises(ValueError):
            sanitize_motion_override(b'{"start_handle": 0.9}')
        with self.assertRaises(ValueError):
            sanitize_motion_override(b'{"peak_speed": true}')
        with self.assertRaises(ValueError):
            sanitize_motion_override(b"not json")

    def test_serializer_emits_one_canonical_shape(self) -> None:
        first = serialize_motion_override({"turn_radius": 90.0, "peak_speed": 800.0})
        second = serialize_motion_override({"peak_speed": 800.0, "turn_radius": 90.0})
        self.assertEqual(first, second)
        parsed = json.loads(first)
        # Only the supplied fields are serialized, in canonical field order.
        self.assertEqual(list(parsed), ["schema", "peak_speed", "turn_radius"])
        self.assertEqual(parsed["schema"], "cua.cursor-gallery-motion-override/1")


if __name__ == "__main__":
    unittest.main()
