"""
Tests for Computer.tracing functionality.
"""

import pytest
import asyncio
import tempfile
import json
from pathlib import Path
from computer.tracing import ComputerTracing


class MockComputer:
    """Mock computer for testing tracing functionality."""
    
    def __init__(self):
        self.os_type = "macos"
        self.provider_type = "lume"
        self.image = "test-image"
        self.interface = MockInterface()
        self.logger = MockLogger()


class MockInterface:
    """Mock interface for testing."""
    
    async def screenshot(self):
        """Return mock screenshot data."""
        return b"mock_screenshot_data"
        
    async def get_accessibility_tree(self):
        """Return mock accessibility tree."""
        return {"type": "window", "children": []}


class MockLogger:
    """Mock logger for testing."""
    
    def warning(self, message):
        print(f"Warning: {message}")


@pytest.mark.asyncio
async def test_tracing_start_stop():
    """Test basic start and stop functionality."""
    computer = MockComputer()
    tracing = ComputerTracing(computer)
    
    # Test initial state
    assert not tracing.is_tracing
    
    # Start tracing
    with tempfile.TemporaryDirectory() as temp_dir:
        await tracing.start({
            'screenshots': True,
            'api_calls': True,
            'path': temp_dir
        })
        
        # Test tracing is active
        assert tracing.is_tracing
        
        # Stop tracing
        trace_path = await tracing.stop({'format': 'dir'})
        
        # Test tracing is stopped
        assert not tracing.is_tracing
        
        # Verify trace directory exists
        assert Path(trace_path).exists()
        
        # Verify metadata file exists
        metadata_file = Path(trace_path) / "trace_metadata.json"
        assert metadata_file.exists()
        
        # Verify metadata content
        with open(metadata_file) as f:
            metadata = json.load(f)
            assert 'trace_id' in metadata
            assert 'config' in metadata
            assert 'start_time' in metadata
            assert 'end_time' in metadata


@pytest.mark.asyncio
async def test_tracing_api_call_recording():
    """Test API call recording functionality."""
    computer = MockComputer()
    tracing = ComputerTracing(computer)
    
    with tempfile.TemporaryDirectory() as temp_dir:
        await tracing.start({
            'api_calls': True,
            'screenshots': False,
            'path': temp_dir
        })
        
        # Record an API call
        await tracing.record_api_call(
            'left_click', 
            {'x': 100, 'y': 200}, 
            result=None, 
            error=None
        )
        
        # Record another API call with error
        test_error = Exception("Test error")
        await tracing.record_api_call(
            'type_text',
            {'text': 'test'},
            result=None,
            error=test_error
        )
        
        trace_path = await tracing.stop({'format': 'dir'})
        
        # Verify event files were created
        trace_dir = Path(trace_path)
        event_files = list(trace_dir.glob("event_*_api_call.json"))
        assert len(event_files) >= 2
        
        # Verify event content
        with open(event_files[0]) as f:
            event = json.load(f)
            assert event['type'] == 'api_call'
            assert event['data']['method'] == 'left_click'
            assert event['data']['success'] is True


@pytest.mark.asyncio
async def test_tracing_metadata():
    """Test metadata recording functionality."""
    computer = MockComputer()
    tracing = ComputerTracing(computer)
    
    with tempfile.TemporaryDirectory() as temp_dir:
        await tracing.start({
            'metadata': True,
            'path': temp_dir
        })
        
        # Add custom metadata
        await tracing.add_metadata('test_key', 'test_value')
        await tracing.add_metadata('numeric_key', 42)
        await tracing.add_metadata('complex_key', {'nested': 'data'})
        
        trace_path = await tracing.stop({'format': 'dir'})
        
        # Verify metadata event files
        trace_dir = Path(trace_path)
        metadata_files = list(trace_dir.glob("event_*_metadata.json"))
        assert len(metadata_files) >= 3


@pytest.mark.asyncio
async def test_tracing_screenshots():
    """Test screenshot recording functionality."""
    computer = MockComputer()
    tracing = ComputerTracing(computer)
    
    with tempfile.TemporaryDirectory() as temp_dir:
        await tracing.start({
            'screenshots': True,
            'path': temp_dir
        })
        
        # Take a screenshot manually
        await tracing._take_screenshot('manual_test')
        
        trace_path = await tracing.stop({'format': 'dir'})
        
        # Verify screenshot files
        trace_dir = Path(trace_path)
        screenshot_files = list(trace_dir.glob("*.png"))
        assert len(screenshot_files) >= 2  # Initial + manual + final


@pytest.mark.asyncio
async def test_tracing_config_options():
    """Test different configuration options."""
    computer = MockComputer()
    tracing = ComputerTracing(computer)
    
    # Test with minimal config
    with tempfile.TemporaryDirectory() as temp_dir:
        await tracing.start({
            'screenshots': False,
            'api_calls': False,
            'metadata': False,
            'path': temp_dir
        })
        
        await tracing.record_api_call('test_call', {})
        await tracing.add_metadata('test', 'value')
        
        trace_path = await tracing.stop({'format': 'dir'})
        
        # With everything disabled, should only have basic trace events
        trace_dir = Path(trace_path)
        event_files = list(trace_dir.glob("event_*.json"))
        # Should have trace_start and trace_end events only
        assert len(event_files) == 2


@pytest.mark.asyncio
async def test_tracing_zip_output():
    """Test zip file output format."""
    computer = MockComputer()
    tracing = ComputerTracing(computer)
    
    with tempfile.TemporaryDirectory() as temp_dir:
        await tracing.start({
            'screenshots': True,
            'api_calls': True,
            'path': temp_dir
        })
        
        await tracing.record_api_call('test_call', {'arg': 'value'})
        
        # Stop with zip format
        trace_path = await tracing.stop({'format': 'zip'})
        
        # Verify zip file exists
        assert Path(trace_path).exists()
        assert trace_path.endswith('.zip')


@pytest.mark.asyncio 
async def test_tracing_accessibility_tree():
    """Test accessibility tree recording."""
    computer = MockComputer()
    tracing = ComputerTracing(computer)
    
    with tempfile.TemporaryDirectory() as temp_dir:
        await tracing.start({
            'accessibility_tree': True,
            'path': temp_dir
        })
        
        # Record accessibility tree
        await tracing.record_accessibility_tree()
        
        trace_path = await tracing.stop({'format': 'dir'})
        
        # Verify accessibility tree event
        trace_dir = Path(trace_path)
        tree_files = list(trace_dir.glob("event_*_accessibility_tree.json"))
        assert len(tree_files) >= 1
        
        # Verify content
        with open(tree_files[0]) as f:
            event = json.load(f)
            assert event['type'] == 'accessibility_tree'
            assert 'tree' in event['data']


def test_tracing_errors():
    """Test error handling in tracing."""
    computer = MockComputer()
    tracing = ComputerTracing(computer)
    
    # Test stop without start
    with pytest.raises(RuntimeError, match="Tracing is not active"):
        asyncio.run(tracing.stop())
    
    # Test start when already started
    async def test_double_start():
        await tracing.start()
        with pytest.raises(RuntimeError, match="Tracing is already active"):
            await tracing.start()
        await tracing.stop()
    
    asyncio.run(test_double_start())


if __name__ == "__main__":
    # Run tests directly
    import sys
    
    async def run_tests():
        """Run all tests manually."""
        tests = [
            test_tracing_start_stop,
            test_tracing_api_call_recording,
            test_tracing_metadata,
            test_tracing_screenshots,
            test_tracing_config_options,
            test_tracing_zip_output,
            test_tracing_accessibility_tree
        ]
        
        print("Running Computer.tracing tests...")
        
        for test in tests:
            try:
                await test()
                print(f"✓ {test.__name__}")
            except Exception as e:
                print(f"✗ {test.__name__}: {e}")
                
        # Run sync tests
        try:
            test_tracing_errors()
            print("✓ test_tracing_errors")
        except Exception as e:
            print(f"✗ test_tracing_errors: {e}")
            
        print("Tests completed!")
    
    asyncio.run(run_tests())