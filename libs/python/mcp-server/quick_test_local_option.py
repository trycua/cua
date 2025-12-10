#!/usr/bin/env python3
"""
Quick test to verify the local desktop option logic without full setup.

This script tests the environment variable parsing and logic flow
without requiring VMs, computer-server, or MCP clients to be running.
"""

import os
import sys


def test_env_var_parsing():
    """Test that environment variable is parsed correctly."""
    print("Testing CUA_USE_HOST_COMPUTER_SERVER environment variable parsing...")
    print("-" * 60)

    test_cases = [
        # (env_value, expected_result, description)
        ("true", True, "lowercase 'true'"),
        ("True", True, "capitalized 'True'"),
        ("TRUE", True, "uppercase 'TRUE'"),
        ("1", True, "numeric '1'"),
        ("yes", True, "lowercase 'yes'"),
        ("Yes", True, "capitalized 'Yes'"),
        ("false", False, "lowercase 'false'"),
        ("False", False, "capitalized 'False'"),
        ("FALSE", False, "uppercase 'FALSE'"),
        ("0", False, "numeric '0'"),
        ("no", False, "lowercase 'no'"),
        ("", False, "empty string"),
        ("random", False, "random value"),
        (None, False, "not set (None)"),
    ]

    passed = 0
    failed = 0

    for env_value, expected, description in test_cases:
        # Simulate the logic from session_manager.py line 59
        if env_value is None:
            actual = os.getenv("CUA_USE_HOST_COMPUTER_SERVER", "false").lower() in (
                "true",
                "1",
                "yes",
            )
        else:
            os.environ["CUA_USE_HOST_COMPUTER_SERVER"] = env_value
            actual = os.getenv("CUA_USE_HOST_COMPUTER_SERVER", "false").lower() in (
                "true",
                "1",
                "yes",
            )

        status = "✓ PASS" if actual == expected else "✗ FAIL"
        if actual == expected:
            passed += 1
        else:
            failed += 1

        print(
            f"{status} | Value: {env_value!r:15} | Expected: {expected!s:5} | Got: {actual!s:5} | {description}"
        )

    # Clean up
    os.environ.pop("CUA_USE_HOST_COMPUTER_SERVER", None)

    print("-" * 60)
    print(f"Results: {passed} passed, {failed} failed")
    return failed == 0


def test_session_manager_logic():
    """Test the logic flow in session_manager.py without actual Computer creation."""
    print("\nTesting session_manager.py logic flow...")
    print("-" * 60)

    # Read the actual session_manager.py to verify the logic
    import pathlib

    session_manager_path = (
        pathlib.Path(__file__).parent.parent
        / "libs"
        / "python"
        / "mcp-server"
        / "mcp_server"
        / "session_manager.py"
    )

    if not session_manager_path.exists():
        print(f"✗ FAIL | session_manager.py not found at {session_manager_path}")
        return False

    content = session_manager_path.read_text()

    # Check for the key logic
    checks = [
        ('os.getenv("CUA_USE_HOST_COMPUTER_SERVER"', "Environment variable check present"),
        ("use_host_computer_server=use_host", "use_host_computer_server parameter passed"),
        ("Computer(", "Computer instantiation present"),
    ]

    all_checks_passed = True
    for check_str, description in checks:
        if check_str in content:
            print(f"✓ PASS | {description}")
        else:
            print(f"✗ FAIL | {description} - not found")
            all_checks_passed = False

    print("-" * 60)
    return all_checks_passed


def test_documentation_consistency():
    """Verify documentation mentions the new feature."""
    print("\nTesting documentation consistency...")
    print("-" * 60)

    import pathlib

    docs_to_check = [
        ("configuration.mdx", "CUA_USE_HOST_COMPUTER_SERVER"),
        ("usage.mdx", "Targeting Your Local Desktop"),
    ]

    docs_path = (
        pathlib.Path(__file__).parent.parent
        / "docs"
        / "content"
        / "docs"
        / "libraries"
        / "mcp-server"
    )

    all_docs_ok = True
    for doc_file, expected_content in docs_to_check:
        doc_path = docs_path / doc_file
        if not doc_path.exists():
            print(f"✗ FAIL | {doc_file} not found")
            all_docs_ok = False
            continue

        content = doc_path.read_text()
        if expected_content in content:
            print(f"✓ PASS | {doc_file} contains '{expected_content}'")
        else:
            print(f"✗ FAIL | {doc_file} missing '{expected_content}'")
            all_docs_ok = False

    print("-" * 60)
    return all_docs_ok


def print_usage_examples():
    """Print usage examples for both modes."""
    print("\n" + "=" * 60)
    print("USAGE EXAMPLES")
    print("=" * 60)

    print("\n1. DEFAULT MODE (VM):")
    print("-" * 60)
    print(
        """
{
  "mcpServers": {
    "cua-agent": {
      "command": "/bin/bash",
      "args": ["~/.cua/start_mcp_server.sh"],
      "env": {
        "CUA_MODEL_NAME": "anthropic/claude-sonnet-4-5-20250929"
      }
    }
  }
}

Note: CUA_USE_HOST_COMPUTER_SERVER is not set, so VM mode is used (safe).
"""
    )

    print("\n2. LOCAL DESKTOP MODE:")
    print("-" * 60)
    print(
        """
Step 1: Start computer-server locally:
    python -m computer_server

Step 2: Configure MCP client:
{
  "mcpServers": {
    "cua-agent": {
      "command": "/bin/bash",
      "args": ["~/.cua/start_mcp_server.sh"],
      "env": {
        "CUA_MODEL_NAME": "anthropic/claude-sonnet-4-5-20250929",
        "CUA_USE_HOST_COMPUTER_SERVER": "true"
      }
    }
  }
}

⚠️  WARNING: AI will have direct access to your desktop!
"""
    )


def main():
    """Run all quick tests."""
    print("=" * 60)
    print("QUICK TEST: MCP Server Local Desktop Option")
    print("=" * 60)
    print()

    results = []

    # Run tests
    results.append(("Environment Variable Parsing", test_env_var_parsing()))
    results.append(("Session Manager Logic", test_session_manager_logic()))
    results.append(("Documentation Consistency", test_documentation_consistency()))

    # Print summary
    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)
    for test_name, passed in results:
        status = "✓ PASSED" if passed else "✗ FAILED"
        print(f"{status} | {test_name}")

    all_passed = all(result for _, result in results)

    if all_passed:
        print("\n🎉 All quick tests passed!")
        print_usage_examples()
        print("\nNext steps:")
        print("1. Run full automated tests: pytest tests/test_mcp_server_local_option.py")
        print("2. Follow manual testing guide: tests/MANUAL_TEST_LOCAL_OPTION.md")
        return 0
    else:
        print("\n❌ Some tests failed. Please review the output above.")
        return 1


if __name__ == "__main__":
    sys.exit(main())
