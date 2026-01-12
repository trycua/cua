#!/usr/bin/env python3
"""
Test script for the Slack clone environment.

This script validates that:
1. The Slack UI can be launched
2. The global __slack_shell API is available
3. Messages can be sent and retrieved via the API

Requirements:
    pip install cua-bench-ui

Usage:
    1. Start the server: ./start_server.sh 8080
    2. Run this test: python test_slack.py
"""

import time

from cua_bench_ui import execute_javascript, launch_window


def test_slack_message_api():
    """Test the Slack clone's message API."""

    # Launch the Slack UI
    # Note: Update the URL if using a different port or host
    url = "http://localhost:8080"
    pid = launch_window(url=url, title="Slack")

    # Wait for page to load
    time.sleep(2)

    # Verify the API exists
    api_exists = execute_javascript(pid, "typeof window.__slack_shell !== 'undefined'")
    if not api_exists:
        print("FAILED: window.__slack_shell API not found")
        return False

    print("API found: window.__slack_shell")

    # Prompt user to send a message
    input("Task: Send a message containing the text 'Hello!' in the Slack UI, then press Enter...")

    # Get outbound messages via the API
    msgs = execute_javascript(pid, "window.__slack_shell.getOutboundMessages()")

    print(f"Retrieved {len(msgs)} outbound message(s)")

    # Check if any message contains "Hello!"
    passed = any("Hello!" in str(msg.get("text", "")) for msg in msgs)

    if passed:
        print("PASSED: Found message containing 'Hello!'")
    else:
        print("FAILED: No message containing 'Hello!' found")
        print(f"Messages received: {msgs}")

    return passed


def test_slack_api_methods():
    """Test all API methods are available."""

    url = "http://localhost:8080"
    pid = launch_window(url=url, title="Slack API Test")

    time.sleep(2)

    # Test getOutboundMessages
    msgs = execute_javascript(pid, "window.__slack_shell.getOutboundMessages()")
    assert isinstance(msgs, list), "getOutboundMessages should return an array"
    print("getOutboundMessages(): OK")

    # Test getAllMessages
    all_msgs = execute_javascript(pid, "window.__slack_shell.getAllMessages()")
    assert isinstance(all_msgs, list), "getAllMessages should return an array"
    print("getAllMessages(): OK")

    # Test getCurrentChannel
    channel = execute_javascript(pid, "window.__slack_shell.getCurrentChannel()")
    assert channel == "general", f"Expected 'general', got '{channel}'"
    print("getCurrentChannel(): OK")

    print("\nAll API methods working correctly!")
    return True


if __name__ == "__main__":
    import sys

    print("=" * 50)
    print("Slack Clone Test Suite")
    print("=" * 50)
    print()

    # Run the main test
    passed = test_slack_message_api()

    print()
    print("=" * 50)
    print(f"Result: {'PASSED' if passed else 'FAILED'}")
    print("=" * 50)

    sys.exit(0 if passed else 1)
