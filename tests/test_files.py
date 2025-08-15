"""
File System Interface Tests
Tests for the file system methods of the Computer interface (macOS).
Required environment variables:
- CUA_API_KEY: API key for Cua cloud provider
- CUA_CONTAINER_NAME: Name of the container to use
"""

import os
import asyncio
import pytest
from pathlib import Path
import sys
import traceback

# Load environment variables from .env file
project_root = Path(__file__).parent.parent
env_file = project_root / ".env"
print(f"Loading environment from: {env_file}")
from dotenv import load_dotenv

load_dotenv(env_file)

# Add paths to sys.path if needed
pythonpath = os.environ.get("PYTHONPATH", "")
for path in pythonpath.split(":"):
    if path and path not in sys.path:
        sys.path.insert(0, path)  # Insert at beginning to prioritize
        print(f"Added to sys.path: {path}")

from computer import Computer, VMProviderType

@pytest.fixture(scope="session")
async def computer():
    """Shared Computer instance for all test cases."""
    # Create a remote Linux computer with Cua
    computer = Computer(
        os_type="linux",
        api_key=os.getenv("CUA_API_KEY"),
        name=str(os.getenv("CUA_CONTAINER_NAME")),
        provider_type=VMProviderType.CLOUD,
    )
    
    # Create a local macOS computer with Cua
    # computer = Computer()
    
    # Connect to host computer
    # computer = Computer(use_host_computer_server=True)
    
    try:
        await computer.run()
        yield computer
    finally:
        await computer.disconnect()

@pytest.mark.asyncio(loop_scope="session")
async def test_file_exists(computer):
    tmp_path = "test_file_exists.txt"
    # Ensure file does not exist
    if await computer.interface.file_exists(tmp_path):
        await computer.interface.delete_file(tmp_path)
    exists = await computer.interface.file_exists(tmp_path)
    assert exists is False, f"File {tmp_path} should not exist"
    # Create file and check again
    await computer.interface.write_text(tmp_path, "hello")
    exists = await computer.interface.file_exists(tmp_path)
    assert exists is True, f"File {tmp_path} should exist"
    await computer.interface.delete_file(tmp_path)


@pytest.mark.asyncio(loop_scope="session")
async def test_directory_exists(computer):
    tmp_dir = "test_directory_exists"
    if await computer.interface.directory_exists(tmp_dir):
        # Remove all files in directory before removing directory
        files = await computer.interface.list_dir(tmp_dir)
        for fname in files:
            await computer.interface.delete_file(f"{tmp_dir}/{fname}")
        # Remove the directory itself
        await computer.interface.delete_dir(tmp_dir)
    exists = await computer.interface.directory_exists(tmp_dir)
    assert exists is False, f"Directory {tmp_dir} should not exist"
    await computer.interface.create_dir(tmp_dir)
    exists = await computer.interface.directory_exists(tmp_dir)
    assert exists is True, f"Directory {tmp_dir} should exist"
    # Cleanup: remove files and directory
    files = await computer.interface.list_dir(tmp_dir)
    for fname in files:
        await computer.interface.delete_file(f"{tmp_dir}/{fname}")
    await computer.interface.delete_dir(tmp_dir)


@pytest.mark.asyncio(loop_scope="session")
async def test_list_dir(computer):
    tmp_dir = "test_list_dir"
    if not await computer.interface.directory_exists(tmp_dir):
        await computer.interface.create_dir(tmp_dir)
    files = ["foo.txt", "bar.txt"]
    for fname in files:
        await computer.interface.write_text(f"{tmp_dir}/{fname}", "hi")
    result = await computer.interface.list_dir(tmp_dir)
    assert set(result) >= set(files), f"Directory {tmp_dir} should contain files {files}"
    for fname in files:
        await computer.interface.delete_file(f"{tmp_dir}/{fname}")
    await computer.interface.delete_dir(tmp_dir)


@pytest.mark.asyncio(loop_scope="session")
async def test_read_write_text(computer):
    tmp_path = "test_rw_text.txt"
    content = "sample text"
    await computer.interface.write_text(tmp_path, content)
    read = await computer.interface.read_text(tmp_path)
    assert read == content, "File content should match"
    await computer.interface.delete_file(tmp_path)


@pytest.mark.asyncio(loop_scope="session")
async def test_delete_file(computer):
    tmp_path = "test_delete_file.txt"
    await computer.interface.write_text(tmp_path, "bye")
    exists = await computer.interface.file_exists(tmp_path)
    assert exists is True, "File should exist"
    await computer.interface.delete_file(tmp_path)
    exists = await computer.interface.file_exists(tmp_path)
    assert exists is False, "File should not exist"


@pytest.mark.asyncio(loop_scope="session")
async def test_create_dir(computer):
    tmp_dir = "test_create_dir"
    if await computer.interface.directory_exists(tmp_dir):
        await computer.interface.delete_dir(tmp_dir)
    await computer.interface.create_dir(tmp_dir)
    exists = await computer.interface.directory_exists(tmp_dir)
    assert exists is True, "Directory should exist"
    await computer.interface.delete_dir(tmp_dir)


@pytest.mark.asyncio(loop_scope="session")
async def test_read_bytes_basic(computer):
    """Test basic read_bytes functionality."""
    tmp_path = "test_read_bytes.bin"
    test_data = b"Hello, World! This is binary data \x00\x01\x02\x03"
    
    # Write binary data using write_text (assuming it handles bytes)
    await computer.interface.write_text(tmp_path, test_data.decode('latin-1'))
    
    # Read all bytes
    read_data = await computer.interface.read_bytes(tmp_path)
    assert read_data == test_data, "Binary data should match"
    
    await computer.interface.delete_file(tmp_path)


@pytest.mark.asyncio(loop_scope="session")
async def test_read_bytes_with_offset_and_length(computer):
    """Test read_bytes with offset and length parameters."""
    tmp_path = "test_read_bytes_offset.bin"
    test_data = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    
    # Write test data
    await computer.interface.write_text(tmp_path, test_data.decode('latin-1'))
    
    # Test reading with offset only
    read_data = await computer.interface.read_bytes(tmp_path, offset=5)
    expected = test_data[5:]
    assert read_data == expected, f"Data from offset 5 should match. Got: {read_data}, Expected: {expected}"
    
    # Test reading with offset and length
    read_data = await computer.interface.read_bytes(tmp_path, offset=10, length=5)
    expected = test_data[10:15]
    assert read_data == expected, f"Data from offset 10, length 5 should match. Got: {read_data}, Expected: {expected}"
    
    # Test reading from beginning with length
    read_data = await computer.interface.read_bytes(tmp_path, offset=0, length=10)
    expected = test_data[:10]
    assert read_data == expected, f"Data from beginning, length 10 should match. Got: {read_data}, Expected: {expected}"
    
    await computer.interface.delete_file(tmp_path)


@pytest.mark.asyncio(loop_scope="session")
async def test_get_file_size(computer):
    """Test get_file_size functionality."""
    tmp_path = "test_file_size.txt"
    test_content = "A" * 1000  # 1000 bytes
    
    await computer.interface.write_text(tmp_path, test_content)
    
    file_size = await computer.interface.get_file_size(tmp_path)
    assert file_size == 1000, f"File size should be 1000 bytes, got {file_size}"
    
    await computer.interface.delete_file(tmp_path)


@pytest.mark.asyncio(loop_scope="session")
async def test_read_large_file(computer):
    """Test reading a file larger than 10MB to verify chunked reading."""
    tmp_path = "test_large_file.bin"
    
    # Create a file larger than 10MB (10 * 1024 * 1024 = 10,485,760 bytes)
    total_size = 12 * 1024 * 1024  # 12MB
    
    print(f"Creating large file of {total_size} bytes ({total_size / (1024*1024):.1f}MB)...")
    
    # Create large file content (this will test the chunked writing functionality)
    large_content = b"X" * total_size
    
    # Write the large file using write_bytes (will automatically use chunked writing)
    await computer.interface.write_bytes(tmp_path, large_content)
    
    # Verify file size
    file_size = await computer.interface.get_file_size(tmp_path)
    assert file_size == total_size, f"Large file size should be {total_size} bytes, got {file_size}"
    
    print(f"Large file created successfully: {file_size} bytes")
    
    # Test reading the entire large file (should use chunked reading)
    print("Reading large file...")
    read_data = await computer.interface.read_bytes(tmp_path)
    assert len(read_data) == total_size, f"Read data size should match file size. Got {len(read_data)}, expected {total_size}"
    
    # Verify content (should be all 'X' characters)
    expected_data = b"X" * total_size
    assert read_data == expected_data, "Large file content should be all 'X' characters"
    
    print("Large file read successfully!")
    
    # Test reading with offset and length on large file
    offset = 5 * 1024 * 1024  # 5MB offset
    length = 2 * 1024 * 1024  # 2MB length
    read_data = await computer.interface.read_bytes(tmp_path, offset=offset, length=length)
    assert len(read_data) == length, f"Partial read size should be {length}, got {len(read_data)}"
    assert read_data == b"X" * length, "Partial read content should be all 'X' characters"
    
    print("Large file partial read successful!")
    
    # Clean up
    await computer.interface.delete_file(tmp_path)
    print("Large file test completed successfully!")

@pytest.mark.asyncio(loop_scope="session")
async def test_read_write_text_with_encoding(computer):
    """Test reading and writing text files with different encodings."""
    print("Testing text file operations with different encodings...")
    
    tmp_path = "test_encoding.txt"
    
    # Test UTF-8 encoding (default)
    utf8_content = "Hello, 世界! 🌍 Ñoño café"
    await computer.interface.write_text(tmp_path, utf8_content, encoding='utf-8')
    read_utf8 = await computer.interface.read_text(tmp_path, encoding='utf-8')
    assert read_utf8 == utf8_content, "UTF-8 content should match"
    
    # Test ASCII encoding
    ascii_content = "Hello, World! Simple ASCII text."
    await computer.interface.write_text(tmp_path, ascii_content, encoding='ascii')
    read_ascii = await computer.interface.read_text(tmp_path, encoding='ascii')
    assert read_ascii == ascii_content, "ASCII content should match"
    
    # Test Latin-1 encoding
    latin1_content = "Café, naïve, résumé"
    await computer.interface.write_text(tmp_path, latin1_content, encoding='latin-1')
    read_latin1 = await computer.interface.read_text(tmp_path, encoding='latin-1')
    assert read_latin1 == latin1_content, "Latin-1 content should match"
    
    # Clean up
    await computer.interface.delete_file(tmp_path)
    print("Text encoding test completed successfully!")

@pytest.mark.asyncio(loop_scope="session")
async def test_write_text_append_mode(computer):
    """Test appending text to files."""
    print("Testing text file append mode...")
    
    tmp_path = "test_append.txt"
    
    # Write initial content
    initial_content = "First line\n"
    await computer.interface.write_text(tmp_path, initial_content)
    
    # Append more content
    append_content = "Second line\n"
    await computer.interface.write_text(tmp_path, append_content, append=True)
    
    # Read and verify
    final_content = await computer.interface.read_text(tmp_path)
    expected_content = initial_content + append_content
    assert final_content == expected_content, f"Expected '{expected_content}', got '{final_content}'"
    
    # Append one more line
    third_content = "Third line\n"
    await computer.interface.write_text(tmp_path, third_content, append=True)
    
    # Read and verify final result
    final_content = await computer.interface.read_text(tmp_path)
    expected_content = initial_content + append_content + third_content
    assert final_content == expected_content, f"Expected '{expected_content}', got '{final_content}'"
    
    # Clean up
    await computer.interface.delete_file(tmp_path)
    print("Text append test completed successfully!")

@pytest.mark.asyncio(loop_scope="session")
async def test_large_text_file(computer):
    """Test reading and writing large text files (>5MB) to verify chunked operations."""
    print("Testing large text file operations...")
    
    tmp_path = "test_large_text.txt"
    
    # Create a large text content (approximately 6MB)
    # Each line is about 100 characters, so 60,000 lines ≈ 6MB
    line_template = "This is line {:06d} with some additional text to make it longer and reach about 100 chars.\n"
    large_content = ""
    num_lines = 60000
    
    print(f"Generating large text content with {num_lines} lines...")
    for i in range(num_lines):
        large_content += line_template.format(i)
    
    content_size_mb = len(large_content.encode('utf-8')) / (1024 * 1024)
    print(f"Generated text content size: {content_size_mb:.2f} MB")
    
    # Write the large text file
    print("Writing large text file...")
    await computer.interface.write_text(tmp_path, large_content)
    
    # Read the entire file back
    print("Reading large text file...")
    read_content = await computer.interface.read_text(tmp_path)
    
    # Verify content matches
    assert read_content == large_content, "Large text file content should match exactly"
    
    # Test partial reading by reading as bytes and decoding specific portions
    print("Testing partial text reading...")
    
    # Read first 1000 characters worth of bytes
    first_1000_chars = large_content[:1000]
    first_1000_bytes = first_1000_chars.encode('utf-8')
    read_bytes = await computer.interface.read_bytes(tmp_path, offset=0, length=len(first_1000_bytes))
    decoded_partial = read_bytes.decode('utf-8')
    assert decoded_partial == first_1000_chars, "Partial text reading should match"
    
    # Test appending to large file
    print("Testing append to large text file...")
    append_text = "\n--- APPENDED CONTENT ---\nThis content was appended to the large file.\n"
    await computer.interface.write_text(tmp_path, append_text, append=True)
    
    # Read and verify appended content
    final_content = await computer.interface.read_text(tmp_path)
    expected_final = large_content + append_text
    assert final_content == expected_final, "Appended large text file should match"
    
    # Clean up
    await computer.interface.delete_file(tmp_path)
    print("Large text file test completed successfully!")

@pytest.mark.asyncio(loop_scope="session")
async def test_text_file_edge_cases(computer):
    """Test edge cases for text file operations."""
    print("Testing text file edge cases...")
    
    tmp_path = "test_edge_cases.txt"
    
    # Test empty file
    empty_content = ""
    await computer.interface.write_text(tmp_path, empty_content)
    read_empty = await computer.interface.read_text(tmp_path)
    assert read_empty == empty_content, "Empty file should return empty string"
    
    # Test file with only whitespace
    whitespace_content = "   \n\t\r\n   \n"
    await computer.interface.write_text(tmp_path, whitespace_content)
    read_whitespace = await computer.interface.read_text(tmp_path)
    assert read_whitespace == whitespace_content, "Whitespace content should be preserved"
    
    # Test file with special characters and newlines
    special_content = "Line 1\nLine 2\r\nLine 3\tTabbed\nSpecial: !@#$%^&*()\n"
    await computer.interface.write_text(tmp_path, special_content)
    read_special = await computer.interface.read_text(tmp_path)
    assert read_special == special_content, "Special characters should be preserved"
    
    # Test very long single line (no newlines)
    long_line = "A" * 10000  # 10KB single line
    await computer.interface.write_text(tmp_path, long_line)
    read_long_line = await computer.interface.read_text(tmp_path)
    assert read_long_line == long_line, "Long single line should be preserved"
    
    # Clean up
    await computer.interface.delete_file(tmp_path)
    print("Text file edge cases test completed successfully!")

if __name__ == "__main__":
    # Run tests directly
    pytest.main([__file__, "-v"])
