from unittest.mock import patch

import pytest
from cua_bench.iconify import clear_cache, process_icons

# Mock SVG response for testing
MOCK_SVG = (
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>'
)


class TestIconify:
    """Test suite for iconify icon processing."""

    def setup_method(self):
        """Clear cache before each test."""
        clear_cache()

    @patch("cua_bench.iconify._fetch_icon_svg")
    def test_basic_iconify_processing(self, mock_fetch):
        """Test basic iconify-icon element processing."""
        mock_fetch.return_value = MOCK_SVG
        html = '<iconify-icon icon="eva:people-outline"></iconify-icon>'
        result = process_icons(html)

        # Should contain SVG content
        assert "<svg" in result
        assert "</svg>" in result
        assert "iconify-icon" not in result

    @patch("cua_bench.iconify._fetch_icon_svg")
    def test_iconify_with_attributes(self, mock_fetch):
        """Test iconify-icon with width and height attributes."""
        mock_fetch.return_value = MOCK_SVG
        html = '<iconify-icon icon="mdi:play" width="24" height="24"></iconify-icon>'
        result = process_icons(html)

        # Should contain SVG with width and height
        assert "<svg" in result
        assert 'width="24"' in result
        assert 'height="24"' in result

    @patch("cua_bench.iconify._fetch_icon_svg")
    def test_iconify_with_class(self, mock_fetch):
        """Test iconify-icon with class attribute."""
        mock_fetch.return_value = MOCK_SVG
        html = '<iconify-icon icon="mdi:play" class="text-blue-500"></iconify-icon>'
        result = process_icons(html)

        # Should contain SVG with class
        assert "<svg" in result
        assert "text-blue-500" in result

    @patch("cua_bench.iconify._fetch_icon_svg")
    def test_multiple_iconify_elements(self, mock_fetch):
        """Test processing multiple iconify-icon elements."""
        mock_fetch.return_value = MOCK_SVG
        html = """
        <div>
            <iconify-icon icon="eva:people-outline"></iconify-icon>
            <iconify-icon icon="mdi:play"></iconify-icon>
        </div>
        """
        result = process_icons(html)

        # Should contain multiple SVGs
        svg_count = result.count("<svg")
        assert svg_count == 2
        assert "iconify-icon" not in result

    def test_empty_html(self):
        """Test with empty HTML."""
        result = process_icons("")
        assert result == ""

    def test_html_without_icons(self):
        """Test HTML without iconify-icon elements."""
        html = '<div class="container"><p>Hello World</p></div>'
        result = process_icons(html)
        assert result == html

    def test_invalid_icon_name(self):
        """Test with invalid icon name."""
        html = '<iconify-icon icon="invalid:nonexistent"></iconify-icon>'
        result = process_icons(html)

        # Should remove the element if icon cannot be resolved
        assert result == ""

    def test_missing_icon_attribute(self):
        """Test iconify-icon without icon attribute."""
        html = '<iconify-icon class="test"></iconify-icon>'
        result = process_icons(html)

        # Should remove the element if no icon specified
        assert result == ""

    @patch("cua_bench.iconify._fetch_icon_svg")
    def test_ignore_iconset_option(self, mock_fetch):
        """Test ignore_iconset option for randomization."""
        mock_fetch.return_value = MOCK_SVG
        html = '<iconify-icon icon="eva:people-outline"></iconify-icon>'

        # Process with ignore_iconset=True
        result = process_icons(html, ignore_iconset=True)

        # Should still process the icon (may use different iconset)
        assert "<svg" in result

    @patch("cua_bench.iconify._fetch_icon_svg")
    def test_colon_to_slash_conversion(self, mock_fetch):
        """Test that colon notation is converted to slash notation."""
        mock_fetch.return_value = MOCK_SVG
        html = '<iconify-icon icon="mdi:play"></iconify-icon>'
        result = process_icons(html)

        # Should process successfully (internal conversion from mdi:play to mdi/play)
        assert "<svg" in result


if __name__ == "__main__":
    pytest.main([__file__])
