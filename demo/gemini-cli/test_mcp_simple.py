#!/usr/bin/env python3
"""Simple script to extract MCP tool schemas and test basic calls"""

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
    exe_path = r"O:\projects\cua-oss-public-cuadriver\libs\cua-driver\rust\target\release\cua-driver.exe"
    output_file = r"O:\projects\cua-oss-public-cuadriver\demo\gemini-cli\mcp_schemas_full.json"
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
        # Initialize
        print("Initializing...")
        init_request = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "schema-extractor", "version": "1.0.0"}
            }
        }
        response = send_mcp_request(process, init_request)
        results["initialize"] = response

        # Send initialized notification
        process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        process.stdin.flush()

        # List all tools
        print("Listing tools...")
        list_tools_request = {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }
        response = send_mcp_request(process, list_tools_request)
        results["tools_list_full"] = response

        if response and "result" in response:
            tools = response["result"].get("tools", [])
            print(f"Found {len(tools)} tools")

            # Save all tools
            results["all_tools"] = tools

            # Extract key tools
            key_names = ["list_windows", "get_window_state", "click", "type_text", "press_key",
                        "screenshot", "get_cursor_position", "move_cursor", "drag", "double_click",
                        "right_click", "scroll", "launch_app"]

            results["key_tools_detailed"] = {}
            for tool in tools:
                if tool["name"] in key_names:
                    results["key_tools_detailed"][tool["name"]] = tool
                    print(f"  - {tool['name']}")

        # Test list_windows
        print("\nTesting list_windows...")
        response = send_mcp_request(process, {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "list_windows", "arguments": {}}
        })

        results["list_windows_test"] = {
            "request": {"name": "list_windows", "arguments": {}},
            "response": response
        }

        if response and "result" in response:
            content = response["result"].get("content", [])
            if content:
                print(f"  Response type: {content[0].get('type')}")
                print(f"  Response text (first 300 chars): {content[0].get('text', '')[:300]}")

        # Test get_cursor_position (no args needed)
        print("\nTesting get_cursor_position...")
        response = send_mcp_request(process, {
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {"name": "get_cursor_position", "arguments": {}}
        })

        results["get_cursor_position_test"] = {
            "request": {"name": "get_cursor_position", "arguments": {}},
            "response": response
        }

        if response and "result" in response:
            content = response["result"].get("content", [])
            if content:
                print(f"  Response: {content[0].get('text', '')}")

        # Save to file
        with open(output_file, 'w') as f:
            json.dump(results, f, indent=2)

        print(f"\nResults saved to: {output_file}")
        print("\nKey findings:")
        print("- MCP tools return responses in 'content' array with 'type' and 'text' fields")
        print("- Some responses are human-readable text, some may be JSON")

    except Exception as e:
        print(f"Error: {e}")
        import traceback
        traceback.print_exc()
    finally:
        process.terminate()
        process.wait()

if __name__ == "__main__":
    main()
