#!/usr/bin/env python3
"""Test script to extract exact MCP tool schemas from cua-driver"""

import json
import subprocess
import sys

def send_mcp_request(process, request):
    """Send a JSON-RPC request to the MCP server"""
    request_line = json.dumps(request) + "\n"
    process.stdin.write(request_line)
    process.stdin.flush()

    # Read response
    response_line = process.stdout.readline()
    if not response_line:
        return None
    return json.loads(response_line)

def main():
    # Start the MCP server
    exe_path = r"O:\projects\cua-oss-public-cuadriver\libs\cua-driver\rust\target\release\cua-driver.exe"

    print("Starting cua-driver MCP server...")
    process = subprocess.Popen(
        [exe_path, "mcp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1
    )

    try:
        # 1. Initialize the connection
        print("\n=== Initializing MCP connection ===")
        init_request = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "schema-extractor",
                    "version": "1.0.0"
                }
            }
        }
        response = send_mcp_request(process, init_request)
        print(f"Initialize response: {json.dumps(response, indent=2)}")

        # 2. Send initialized notification
        initialized_notif = {
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }
        process.stdin.write(json.dumps(initialized_notif) + "\n")
        process.stdin.flush()

        # 3. List all tools
        print("\n=== Listing all tools ===")
        list_tools_request = {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }
        response = send_mcp_request(process, list_tools_request)
        print(f"Tools list: {json.dumps(response, indent=2)}")

        if response and "result" in response:
            tools = response["result"].get("tools", [])
            print(f"\nFound {len(tools)} tools:")
            for tool in tools:
                print(f"  - {tool['name']}")

        # 4. Test list_windows
        print("\n=== Testing list_windows ===")
        list_windows_request = {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "list_windows",
                "arguments": {}
            }
        }
        response = send_mcp_request(process, list_windows_request)
        print(f"list_windows response: {json.dumps(response, indent=2)}")

        # Extract a window for testing
        windows = []
        if response and "result" in response:
            content = response["result"].get("content", [])
            if content:
                windows_data = json.loads(content[0]["text"])
                windows = windows_data.get("windows", [])
                print(f"\nFound {len(windows)} windows")
                if windows:
                    print(f"First window: {json.dumps(windows[0], indent=2)}")

        # 5. Test get_window_state if we have a window
        if windows:
            test_window = windows[0]
            print(f"\n=== Testing get_window_state for window: {test_window.get('title', 'Unknown')} ===")
            get_state_request = {
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "get_window_state",
                    "arguments": {
                        "pid": test_window["pid"],
                        "window_id": test_window["window_id"]
                    }
                }
            }
            response = send_mcp_request(process, get_state_request)

            # Don't print the full response if it has a screenshot (too large)
            if response and "result" in response:
                result_copy = json.loads(json.dumps(response))
                content = result_copy["result"].get("content", [])
                if content:
                    state_data = json.loads(content[0]["text"])
                    if "screenshot" in state_data:
                        screenshot_len = len(state_data["screenshot"])
                        state_data["screenshot"] = f"<base64 data, {screenshot_len} chars>"
                    print(f"get_window_state response: {json.dumps(result_copy, indent=2)}")
                    print(f"\nActual state data keys: {list(state_data.keys())}")
                    if "screenshot" in json.loads(content[0]["text"]):
                        print(f"Screenshot field: base64 string, length {screenshot_len}")

        # 6. Document other tool schemas
        print("\n=== Extracting schemas for key tools ===")
        key_tools = ["click", "type_text", "press_key", "get_cursor_position", "screenshot"]

        for tool_name in key_tools:
            matching_tools = [t for t in tools if t["name"] == tool_name]
            if matching_tools:
                tool = matching_tools[0]
                print(f"\n{tool_name}:")
                print(f"  Description: {tool.get('description', 'N/A')}")
                print(f"  Input schema: {json.dumps(tool.get('inputSchema', {}), indent=4)}")

    finally:
        process.terminate()
        process.wait()

if __name__ == "__main__":
    main()
