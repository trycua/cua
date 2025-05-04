"""Setup script for the Cursor MCP client package."""

from setuptools import setup, find_packages

setup(
    name="cursor-mcp-client",
    version="0.1.0",
    description="Cursor MCP (Model Control Protocol) client for macOS integration",
    author="Cursor Team",
    author_email="team@cursor.sh",
    url="https://github.com/getcursor/cursor",
    packages=find_packages(),
    python_requires=">=3.8",
    install_requires=[
        "asyncio",
        "websockets",
        "pydantic",
    ],
    classifiers=[
        "Development Status :: 3 - Alpha",
        "Intended Audience :: Developers",
        "License :: OSI Approved :: MIT License",
        "Programming Language :: Python :: 3",
        "Programming Language :: Python :: 3.8",
        "Programming Language :: Python :: 3.9",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.11",
    ],
) 