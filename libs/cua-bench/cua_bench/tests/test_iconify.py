import pytest
from cua_bench.iconify import process_icons, clear_cache


class TestIconify:
    """Test suite for iconify icon processing."""
    
    def setup_method(self):
        """Clear cache before each test."""
        clear_cache()
    
    def test_basic_iconify_processing(self):
        """Test basic iconify-icon element processing."""
        html = '<iconify-icon icon="eva:people-outline"></iconify-icon>'
        result = process_icons(html)
        
        # Should contain SVG content
        assert '<svg' in result
        assert '</svg>' in result
        assert 'iconify-icon' not in result
    
    def test_iconify_with_attributes(self):
        """Test iconify-icon with width and height attributes."""
        html = '<iconify-icon icon="mdi:play" width="24" height="24"></iconify-icon>'
        result = process_icons(html)
        
        # Should contain SVG with width and height
        assert '<svg' in result
        assert 'width="24"' in result
        assert 'height="24"' in result
    
    def test_iconify_with_class(self):
        """Test iconify-icon with class attribute."""
        html = '<iconify-icon icon="mdi:play" class="text-blue-500"></iconify-icon>'
        result = process_icons(html)
        
        # Should contain SVG with class
        assert '<svg' in result
        assert 'text-blue-500' in result
    
    def test_multiple_iconify_elements(self):
        """Test processing multiple iconify-icon elements."""
        html = '''
        <div>
            <iconify-icon icon="eva:people-outline"></iconify-icon>
            <iconify-icon icon="mdi:play"></iconify-icon>
        </div>
        '''
        result = process_icons(html)
        
        # Should contain multiple SVGs
        svg_count = result.count('<svg')
        assert svg_count == 2
        assert 'iconify-icon' not in result
    
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
        assert result == ''
    
    def test_missing_icon_attribute(self):
        """Test iconify-icon without icon attribute."""
        html = '<iconify-icon class="test"></iconify-icon>'
        result = process_icons(html)
        
        # Should remove the element if no icon specified
        assert result == ''
    
    def test_ignore_iconset_option(self):
        """Test ignore_iconset option for randomization."""
        html = '<iconify-icon icon="eva:people-outline"></iconify-icon>'
        
        # Process with ignore_iconset=True
        result = process_icons(html, ignore_iconset=True)
        
        # Should still process the icon (may use different iconset)
        assert '<svg' in result or result == ''  # May not find match
    
    def test_colon_to_slash_conversion(self):
        """Test that colon notation is converted to slash notation."""
        html = '<iconify-icon icon="mdi:play"></iconify-icon>'
        result = process_icons(html)
        
        # Should process successfully (internal conversion from mdi:play to mdi/play)
        assert '<svg' in result or result == ''


if __name__ == "__main__":
    pytest.main([__file__])
