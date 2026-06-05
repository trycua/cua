#!/usr/bin/env python3
"""Test to find where screenshot is returned"""

import json
import subprocess

def send_mcp_request(process, request):
    request_line = json.dumps(request) + "\n"
    process.stdin.write(request_line)
    process.stdin.flush()
    response_line = process.stdout.readline()
    if not response_line:
        return None
    return json.loads(response_line)

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
        response = send_mcp_request(process, {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0.0"}
            }
        })

        # Initialized notification
        process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        process.stdin.flush()

        # Launch notepad
        response = send_mcp_request(process, {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "launch_app",
                "arguments": {"name": "notepad"}
            }
        })

        struct_content = response["result"]["structuredContent"]
        pid = struct_content["pid"]
        window_id = struct_content["windows"][0]["window_id"]

        print(f"Testing with pid={pid}, window_id={window_id}")

        # Get window state with vision mode (screenshot only)
        response = send_mcp_request(process, {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "get_window_state",
                "arguments": {
                    "pid": pid,
                    "window_id": window_id,
                    "capture_mode": "vision"
                }
            }
        })

        # Analyze the full response structure
        result = response["result"]
        print(f"\nResponse keys: {list(result.keys())}")

        # Check content array
        if "content" in result:
            print(f"\ncontent array length: {len(result['content'])}")
            for i, item in enumerate(result['content']):
                print(f"\n  content[{i}]:")
                print(f"    type: {item.get('type')}")
                if 'text' in item:
                    print(f"    text length: {len(item['text'])}")
                    print(f"    text preview: {item['text'][:100]}")
                if 'data' in item:
                    print(f"    data length: {len(item['data'])}")
                    print(f"    data prefix: {item['data'][:50]}")
                if 'mimeType' in item:
                    print(f"    mimeType: {item['mimeType']}")
                print(f"    all keys: {list(item.keys())}")

        # Check structuredContent
        if "structuredContent" in result:
            struct = result["structuredContent"]
            print(f"\nstructuredContent keys: {list(struct.keys())}")
            if "screenshot" in struct:
                print(f"  screenshot length: {len(struct['screenshot'])}")
                print(f"  screenshot prefix: {struct['screenshot'][:50]}")

    except Exception as e:
        print(f"Error: {e}")
        import traceback
        traceback.print_exc()
    finally:
        process.terminate()
        process.wait()

if __name__ == "__main__":
    main()
