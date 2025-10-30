#!/usr/bin/env python
"""
Connection test script for Computer Server.

This script tests both WebSocket (/ws) and REST (/cmd) connections to the Computer Server
and keeps it alive, allowing you to verify the server is running correctly.
"""

import argparse
import asyncio
import json
import os
import sys

import aiohttp
import dotenv
import websockets

dotenv.load_dotenv()


async def test_websocket_connection(
    host="localhost", port=8000, keep_alive=False, container_name=None, api_key=None
):
    """Test WebSocket connection to the Computer Server."""
    if container_name:
        # Container mode: use WSS with container domain and port 8443
        uri = f"wss://{container_name}.containers.cloud.trycua.com:8443/ws"
        print(f"Connecting to container {container_name} at {uri}...")
    else:
        # Local mode: use WS with specified host and port
        uri = f"ws://{host}:{port}/ws"
        print(f"Connecting to local server at {uri}...")

    try:
        async with websockets.connect(uri) as websocket:
            print("WebSocket connection established!")

            # If container connection, send authentication first
            if container_name:
                if not api_key:
                    print("Error: API key required for container connections")
                    return False

                print("Sending authentication...")
                auth_message = {
                    "command": "authenticate",
                    "params": {"api_key": api_key, "container_name": container_name},
                }
                await websocket.send(json.dumps(auth_message))
                auth_response = await websocket.recv()
                print(f"Authentication response: {auth_response}")

                # Check if authentication was successful
                auth_data = json.loads(auth_response)
                if not auth_data.get("success", False):
                    print("Authentication failed!")
                    return False
                print("Authentication successful!")

            # Send a test command to get version
            await websocket.send(json.dumps({"command": "version", "params": {}}))
            response = await websocket.recv()
            print(f"Version response: {response}")

            # Send a test command to get screen size
            await websocket.send(json.dumps({"command": "get_screen_size", "params": {}}))
            response = await websocket.recv()
            print(f"Screen size response: {response}")

            if keep_alive:
                print("\nKeeping WebSocket connection alive. Press Ctrl+C to exit...")
                while True:
                    # Send a command every 5 seconds to keep the connection alive
                    await asyncio.sleep(5)
                    await websocket.send(
                        json.dumps({"command": "get_cursor_position", "params": {}})
                    )
                    response = await websocket.recv()
                    print(f"Cursor position: {response}")
    except websockets.exceptions.ConnectionClosed as e:
        print(f"WebSocket connection closed: {e}")
        return False
    except ConnectionRefusedError:
        print(f"Connection refused. Is the server running at {host}:{port}?")
        return False
    except Exception as e:
        print(f"WebSocket error: {e}")
        return False

    return True


async def test_rest_connection(
    host="localhost", port=8000, keep_alive=False, container_name=None, api_key=None
):
    """Test REST connection to the Computer Server."""
    if container_name:
        # Container mode: use HTTPS with container domain and port 8443
        base_url = f"https://{container_name}.containers.cloud.trycua.com:8443"
        print(f"Connecting to container {container_name} at {base_url}...")
    else:
        # Local mode: use HTTP with specified host and port
        base_url = f"http://{host}:{port}"
        print(f"Connecting to local server at {base_url}...")

    try:
        async with aiohttp.ClientSession() as session:
            print("REST connection established!")

            # Prepare headers for container authentication
            headers = {}
            if container_name:
                if not api_key:
                    print("Error: API key required for container connections")
                    return False
                headers["X-Container-Name"] = container_name
                headers["X-API-Key"] = api_key
                print("Using container authentication headers")

            # Test screenshot endpoint
            async with session.post(
                f"{base_url}/cmd", json={"command": "screenshot", "params": {}}, headers=headers
            ) as response:
                if response.status == 200:
                    text = await response.text()
                    print(f"Screenshot response: {text}")
                else:
                    print(f"Screenshot request failed with status: {response.status}")
                    print(await response.text())
                    return False

            # Test screen size endpoint
            async with session.post(
                f"{base_url}/cmd",
                json={"command": "get_screen_size", "params": {}},
                headers=headers,
            ) as response:
                if response.status == 200:
                    text = await response.text()
                    print(f"Screen size response: {text}")
                else:
                    print(f"Screen size request failed with status: {response.status}")
                    print(await response.text())
                    return False

            if keep_alive:
                print("\nKeeping REST connection alive. Press Ctrl+C to exit...")
                while True:
                    # Send a command every 5 seconds to keep testing
                    await asyncio.sleep(5)
                    async with session.post(
                        f"{base_url}/cmd",
                        json={"command": "get_cursor_position", "params": {}},
                        headers=headers,
                    ) as response:
                        if response.status == 200:
                            text = await response.text()
                            print(f"Cursor position: {text}")
                        else:
                            print(f"Cursor position request failed with status: {response.status}")
                            print(await response.text())
                            return False

    except aiohttp.ClientError as e:
        print(f"REST connection error: {e}")
        return False
    except Exception as e:
        print(f"REST error: {e}")
        return False

    return True


async def test_connection(
    host="localhost", port=8000, keep_alive=False, container_name=None, use_rest=False, api_key=None
):
    """Test connection to the Computer Server using WebSocket or REST."""
    if use_rest:
        return await test_rest_connection(host, port, keep_alive, container_name, api_key)
    else:
        return await test_websocket_connection(host, port, keep_alive, container_name, api_key)


def parse_args():
    parser = argparse.ArgumentParser(description="Test connection to Computer Server")
    parser.add_argument("--host", default="localhost", help="Host address (default: localhost)")
    parser.add_argument("-p", "--port", type=int, default=8000, help="Port number (default: 8000)")
    parser.add_argument(
        "-c",
        "--container-name",
        help="Container name for cloud connection (uses WSS/HTTPS and port 8443)",
    )
    parser.add_argument(
        "--api-key", help="API key for container authentication (can also use CUA_API_KEY env var)"
    )
    parser.add_argument("--keep-alive", action="store_true", help="Keep connection alive")
    parser.add_argument(
        "--rest", action="store_true", help="Use REST endpoint (/cmd) instead of WebSocket (/ws)"
    )
    return parser.parse_args()


async def main():
    args = parse_args()

    # Convert hyphenated argument to underscore for function parameter
    container_name = getattr(args, "container_name", None)

    # Get API key from argument or environment variable
    api_key = getattr(args, "api_key", None) or os.environ.get("CUA_API_KEY")

    # Check if container name is provided but API key is missing
    if container_name and not api_key:
        print("Warning: Container name provided but no API key found.")
        print("Please provide --api-key argument or set CUA_API_KEY environment variable.")
        return 1

    print(f"Testing {'REST' if args.rest else 'WebSocket'} connection...")
    if container_name:
        print(f"Container: {container_name}")
        print(
            f"API Key: {'***' + api_key[-4:] if api_key and len(api_key) > 4 else 'Not provided'}"
        )

    success = await test_connection(
        host=args.host,
        port=args.port,
        keep_alive=args.keep_alive,
        container_name=container_name,
        use_rest=args.rest,
        api_key=api_key,
    )
    return 0 if success else 1


if __name__ == "__main__":
    try:
        sys.exit(asyncio.run(main()))
    except KeyboardInterrupt:
        print("\nExiting...")
        sys.exit(0)
