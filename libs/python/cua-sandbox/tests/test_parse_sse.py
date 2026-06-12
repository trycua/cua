"""Unit tests for ``HTTPTransport._parse_sse`` — the error-surfacing path.

When the server returns ``{"success": false, ...}``, ``_parse_sse`` raises
``RuntimeError`` with a message derived from the payload.  The previous
implementation stringified ``payload.get('error', 'unknown')``, which
turned every shell-command failure (``{"success": false, "stdout": "",
"stderr": "Command timed out after 10s", "return_code": 1}``) into the
opaque ``Remote error: unknown`` — hiding the actual cause.

These tests pin the new behavior:
  * explicit ``error`` key still wins
  * shell-command shape surfaces ``return_code`` + ``stderr``
  * stdout only appears as a fallback when stderr is empty
  * successful payloads pass through unchanged
  * an SSE stream with no data frame raises a parseable error
"""

import pytest
from cua_sandbox.transport.http import HTTPTransport


def _sse(obj: str) -> str:
    """Wrap a JSON payload in a single SSE data frame."""
    return f"data: {obj}\n\n"


class TestParseSSESurfacesFailures:
    def test_success_payload_returned_unchanged(self):
        out = HTTPTransport._parse_sse(_sse('{"success": true, "result": 42}'))
        assert out == {"success": True, "result": 42}

    def test_explicit_error_field_preserved_verbatim(self):
        with pytest.raises(RuntimeError, match=r"Remote error: boom"):
            HTTPTransport._parse_sse(_sse('{"success": false, "error": "boom"}'))

    def test_shell_failure_surfaces_return_code_and_stderr(self):
        with pytest.raises(RuntimeError) as exc:
            HTTPTransport._parse_sse(
                _sse(
                    '{"success": false, "stdout": "", '
                    '"stderr": "Command timed out after 10s", '
                    '"return_code": 1}'
                )
            )
        msg = str(exc.value)
        assert "Remote error:" in msg
        assert "return_code=1" in msg
        assert "Command timed out after 10s" in msg

    def test_shell_failure_uses_stdout_only_when_stderr_empty(self):
        with pytest.raises(RuntimeError) as exc:
            HTTPTransport._parse_sse(
                _sse(
                    '{"success": false, "stdout": "partial progress", '
                    '"stderr": "", "return_code": 137}'
                )
            )
        msg = str(exc.value)
        assert "return_code=137" in msg
        assert "partial progress" in msg

    def test_shell_failure_stdout_not_added_when_stderr_present(self):
        """stderr wins over stdout when both are non-empty — avoids bloating
        the message with noisy successful-command output that happened to
        return non-zero."""
        with pytest.raises(RuntimeError) as exc:
            HTTPTransport._parse_sse(
                _sse(
                    '{"success": false, '
                    '"stdout": "UI hierchary dumped to: /sdcard/ui.xml\\n", '
                    '"stderr": "Killed", "return_code": 137}'
                )
            )
        msg = str(exc.value)
        assert "Killed" in msg
        assert "return_code=137" in msg
        assert "UI hierchary dumped" not in msg  # stdout suppressed

    def test_shell_failure_with_no_detail_falls_back_cleanly(self):
        with pytest.raises(RuntimeError, match=r"Remote error: no detail"):
            HTTPTransport._parse_sse(_sse('{"success": false}'))

    def test_missing_data_frame_raises(self):
        with pytest.raises(RuntimeError, match=r"No SSE data frame"):
            HTTPTransport._parse_sse("event: ping\n\n")

    def test_ignores_content_before_data_line(self):
        text = ":comment\nevent: message\ndata: " + '{"success": true, "ok": 1}' + "\n\n"
        out = HTTPTransport._parse_sse(text)
        assert out == {"success": True, "ok": 1}
