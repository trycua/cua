#!/usr/bin/env python3
"""Test get_window_state to see exact response format"""

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
    output_file = r"O:\projects\cua-oss-public-cuadriver\demo\gemini-cli\window_state_test.json"

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
        init_request = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0.0"}
            }
        }
        response = send_mcp_request(process, init_request)

        # Send initialized notification
        process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        process.stdin.flush()

        # Test get_window_state on Notepad (a simple app)
        # First launch notepad
        print("Launching Notepad...")
        response = send_mcp_request(process, {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "launch_app",
                "arguments": {"name": "notepad"}
            }
        })

        print(f"Launch response: {json.dumps(response, indent=2)[:500]}")

        # Extract pid and window_id from structured content
        if response and "result" in response:
            struct_content = response["result"].get("structuredContent", {})
            pid = struct_content.get("pid")
            windows = struct_content.get("windows", [])

            if pid and windows:
                window_id = windows[0]["window_id"]
                print(f"\nNotepad pid={pid}, window_id={window_id}")

                # Now get window state
                print("\nCalling get_window_state...")
                response = send_mcp_request(process, {
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {
                        "name": "get_window_state",
                        "arguments": {
                            "pid": pid,
                            "window_id": window_id,
                            "capture_mode": "som"
                        }
                    }
                })

                # Extract and analyze the response
                if response and "result" in response:
                    result = response["result"]

                    # Save structured response info
                    output = {
                        "response_structure": {
                            "has_content": "content" in result,
                            "has_structuredContent": "structuredContent" in result,
                            "content_type": result.get("content", [{}])[0].get("type") if "content" in result else None
                        }
                    }

                    # Check structuredContent
                    if "structuredContent" in result:
                        struct = result["structuredContent"]
                        output["structuredContent_keys"] = list(struct.keys())

                        # Check for screenshot
                        if "screenshot" in struct:
                            screenshot = struct["screenshot"]
                            output["screenshot_info"] = {
                                "length": len(screenshot),
                                "prefix": screenshot[:50],
                                "is_base64": screenshot.startswith("iVBOR") or screenshot.startswith("/9j/") or screenshot.startswith("UklG")
                            }
                            print(f"Screenshot field: {len(screenshot)} chars, prefix: {screenshot[:50]}")
                        else:
                            output["screenshot_info"] = "NOT FOUND in structuredContent"

                        # Sample the structure (without screenshot)
                        struct_sample = {k: v for k, v in struct.items() if k != "screenshot"}
                        if "screenshot" in struct:
                            struct_sample["screenshot"] = "<base64 data removed>"
                        output["structuredContent_sample"] = struct_sample

                    with open(output_file, 'w') as f:
                        json.dump(output, f, indent=2)

                    print(f"\nResults saved to {output_file}")
                    print(f"\nKey findings:")
                    print(f"  - structuredContent keys: {output['structuredContent_keys']}")

    except Exception as e:
        print(f"Error: {e}")
        import traceback
        traceback.print_exc()
    finally:
        process.terminate()
        process.wait()

if __name__ == "__main__":
    main()
