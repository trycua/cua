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

    output_file = "/o/projects/cua-oss-public-cuadriver/demo/gemini-cli/mcp_schemas_output.json"
    results = {}

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
        results["initialize"] = response
        print("[OK] Initialized")

        # 2. Send initialized notification
        initialized_notif = {
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }
        process.stdin.write(json.dumps(initialized_notif) + "\n")
        process.stdin.flush()

        # 3. List all tools
        print("=== Listing all tools ===")
        list_tools_request = {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }
        response = send_mcp_request(process, list_tools_request)
        results["tools_list"] = response

        if response and "result" in response:
            tools = response["result"].get("tools", [])
            print(f"[OK] Found {len(tools)} tools")

            # Extract key tools
            key_tool_names = ["list_windows", "get_window_state", "click", "type_text", "press_key",
                             "screenshot", "get_cursor_position", "move_cursor", "drag"]
            results["key_tools"] = {}

            for tool in tools:
                if tool["name"] in key_tool_names:
                    results["key_tools"][tool["name"]] = {
                        "name": tool["name"],
                        "description": tool["description"],
                        "inputSchema": tool["inputSchema"],
                        "annotations": tool.get("annotations", {})
                    }
                    print(f"  [OK] Extracted schema for {tool['name']}")

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

        # Store the raw response
        results["list_windows_sample"] = {
            "full_response": response,
            "note": "list_windows returns human-readable text, not JSON"
        }

        print(f"[OK] Got list_windows response")
        if response and "result" in response:
            content = response["result"].get("content", [])
            if content:
                text_content = content[0].get("text", "")
                print(f"Response type: {content[0].get('type')}")
                print(f"Response text (first 500 chars): {text_content[:500]}")

        # 5. Test get_window_state - manually specify a window
        # For testing, let's try to get state of a Chrome window (pid 26452, window_id 67124 from debug output)
        print(f"\n=== Testing get_window_state ===")
        if True:  # Always try this
            # Use a hardcoded window for testing
            test_pid = 26452
            test_window_id = 67124
            get_state_request = {
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "get_window_state",
                    "arguments": {
                        "pid": test_pid,
                        "window_id": test_window_id
                    }
                }
            }
            response = send_mcp_request(process, get_state_request)

            if response and "result" in response:
                content = response["result"].get("content", [])
                if content:
                    state_data = json.loads(content[0]["text"])

                    # Extract screenshot info without storing the full base64
                    screenshot_info = None
                    if "screenshot" in state_data:
                        screenshot = state_data["screenshot"]
                        screenshot_info = {
                            "length": len(screenshot),
                            "prefix": screenshot[:50],
                            "type": "base64_png" if screenshot.startswith("iVBOR") else "base64_unknown"
                        }
                        # Remove for storage
                        state_data_copy = {k: v for k, v in state_data.items() if k != "screenshot"}
                        state_data_copy["screenshot"] = "<removed>"
                    else:
                        state_data_copy = state_data

                    results["get_window_state_sample"] = {
                        "response_structure": {
                            "jsonrpc": response.get("jsonrpc"),
                            "id": response.get("id"),
                            "result": {
                                "content": [{
                                    "type": content[0].get("type"),
                                    "text": "<state_data>"
                                }]
                            }
                        },
                        "state_data_keys": list(state_data.keys()),
                        "state_data_sample": state_data_copy,
                        "screenshot_info": screenshot_info
                    }
                    print(f"[OK] Got window state with keys: {list(state_data.keys())}")
                    if screenshot_info:
                        print(f"  Screenshot: {screenshot_info['length']} chars, type: {screenshot_info['type']}")

        # Save results
        with open(output_file, 'w') as f:
            json.dump(results, f, indent=2)

        print(f"\n[OK] Results saved to {output_file}")

    finally:
        process.terminate()
        process.wait()

if __name__ == "__main__":
    main()
