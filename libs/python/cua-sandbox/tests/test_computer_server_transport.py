import base64

import pytest

from cua_sandbox.transport.computer_server import (
    decode_screenshot_response,
    normalize_screen_size,
    parse_command_response,
)


def test_parse_command_response_returns_sse_payload():
    result = parse_command_response('event: result\ndata: {"success": true, "result": {"ok": 1}}\n\n')

    assert result == {"success": True, "result": {"ok": 1}}


def test_parse_command_response_raises_remote_error():
    with pytest.raises(RuntimeError, match="return_code=2"):
        parse_command_response(
            'data: {"success": false, "return_code": 2, "stderr": "bad command"}\n\n'
        )


def test_parse_command_response_requires_data_frame():
    with pytest.raises(RuntimeError, match="No SSE data frame"):
        parse_command_response("event: keepalive\n\n")


def test_decode_screenshot_response_accepts_nested_payload():
    encoded = base64.b64encode(b"png-data").decode()

    assert decode_screenshot_response({"result": {"image_data": encoded}}) == b"png-data"


def test_normalize_screen_size_accepts_nested_aliases():
    assert normalize_screen_size({"result": {"screen_width": "1280", "screen_height": 720}}) == {
        "width": 1280,
        "height": 720,
    }


def test_normalize_screen_size_rejects_unknown_shape():
    with pytest.raises(KeyError, match="Cannot extract screen size"):
        normalize_screen_size({"result": {"unexpected": True}})
