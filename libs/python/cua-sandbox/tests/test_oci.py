"""The legacy QEMU artifact helpers' reference parser (qemu_builder push/pull).

What an image is (kind, variant, OS) is resolved natively: see
libs/cua/crates/cua-image (resolve, detect) and tests/test_image_resolve.py.
"""

from __future__ import annotations

from cua_sandbox.registry.ref import parse_ref

# ═════════════════════════════════════════════════════════════════════════════
# parse_ref
# ═════════════════════════════════════════════════════════════════════════════


class TestParseRef:
    def test_full_ref(self):
        assert parse_ref("ghcr.io/trycua/macos-sequoia-cua:latest") == (
            "ghcr.io",
            "trycua",
            "macos-sequoia-cua",
            "latest",
        )

    def test_org_name(self):
        assert parse_ref("trycua/cua-xfce:v2") == ("ghcr.io", "trycua", "cua-xfce", "v2")

    def test_short_name(self):
        assert parse_ref("cua-xfce") == ("ghcr.io", "trycua", "cua-xfce", "latest")

    def test_short_name_with_tag(self):
        assert parse_ref("cua-xfce:nightly") == ("ghcr.io", "trycua", "cua-xfce", "nightly")
