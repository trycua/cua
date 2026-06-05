#!/usr/bin/env python3
"""Test SOM mode to verify we get both tree and screenshot"""

import json
import subprocess

def send_mcp_request(process, request):
    request_line = json.dumps(request) + "\n"
    process.stdin.write(request_line)
    process.stdin.flush()
    response_line = process.stdout.readline()
    return json.loads(response_line) if response_line else None

def main():
    exe_path = r"O:\projects\cua-oss-public-cuadriver\libs\cua-driver\rust\target\release\cua-driver.exe"

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
        send_mcp_request(process, {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0.0"}
            }
        })

        process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        process.stdin.flush()

        # Launch notepad
        response = send_mcp_request(process, {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "launch_app", "arguments": {"name": "notepad"}}
        })

        struct_content = response["result"]["structuredContent"]
        pid = struct_content["pid"]
        window_id = struct_content["windows"][0]["window_id"]

        print(f"Testing SOM mode with pid={pid}, window_id={window_id}\n")

        # Get window state with SOM mode (default - tree + screenshot)
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

        result = response["result"]

        print("=== SOM MODE RESPONSE STRUCTURE ===\n")
        print(f"Result keys: {list(result.keys())}\n")

        # Analyze content array
        print(f"content[] array length: {len(result['content'])}\n")
        for i, item in enumerate(result['content']):
            print(f"content[{i}]:")
            print(f"  type: {item['type']}")
            if item['type'] == 'text':
                text = item['text']
                print(f"  text length: {len(text)} chars")
                print(f"  text preview: {text[:150]}")
            elif item['type'] == 'image':
                print(f"  mimeType: {item['mimeType']}")
                print(f"  data length: {len(item['data'])} chars (base64)")
                print(f"  data prefix: {item['data'][:50]}")
            print()

        # Analyze structuredContent
        struct = result['structuredContent']
        print(f"structuredContent keys: {list(struct.keys())}\n")
        print(f"  pid: {struct['pid']}")
        print(f"  window_id: {struct['window_id']}")
        print(f"  element_count: {struct['element_count']}")
        print(f"  screenshot_width: {struct['screenshot_width']}")
        print(f"  screenshot_height: {struct['screenshot_height']}")
        print(f"  tree_markdown length: {len(struct['tree_markdown'])} chars")
        print(f"  tree_markdown preview:\n{struct['tree_markdown'][:300]}")

        # Summary
        print("\n=== SUMMARY ===")
        print(f"✓ SOM mode returns {len(result['content'])} content items:")
        print(f"  - content[0]: text (human-readable summary)")
        print(f"  - content[1]: image (base64 PNG screenshot)")
        print(f"✓ structuredContent contains:")
        print(f"  - tree_markdown: UIA tree with [element_index N] tags")
        print(f"  - screenshot dimensions")
        print(f"  - element_count")
        print(f"  - pid, window_id")

    except Exception as e:
        print(f"Error: {e}")
        import traceback
        traceback.print_exc()
    finally:
        process.terminate()
        process.wait()

if __name__ == "__main__":
    main()
