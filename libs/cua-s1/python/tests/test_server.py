from __future__ import annotations

from cua_s1.pdf import PdfError
from cua_s1.server import structured_errors


def test_structured_errors_redact_paths_and_accessibility_values():
    @structured_errors
    def fail():
        raise PdfError(
            "failed",
            "Failure",
            path="/private/records/person.pdf",
            allowed_roots=["/private/records"],
            response={
                "tree_markdown": '[1] AXTextField "Secret" [value="private"]',
                "element": {"label": "Secret", "value": "private"},
            },
            host_location=r"C:\Users\person\record.pdf",
            safe_count=2,
        )

    error = fail()["error"]

    assert error["details"] == {
        "path": "<redacted>",
        "allowed_roots": "<redacted>",
        "response": "<redacted>",
        "host_location": "<redacted-path>",
        "safe_count": 2,
    }
    assert "/private" not in repr(error)
    assert "Secret" not in repr(error)
