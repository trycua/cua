"""
Example usage of the proxy server and client requests.
"""

import dotenv

dotenv.load_dotenv()

import asyncio
import json
import os
from typing import Any, Dict

import aiohttp


async def test_http_endpoint():
    """Test the HTTP /responses endpoint."""

    anthropic_api_key = os.getenv("ANTHROPIC_API_KEY")
    assert isinstance(anthropic_api_key, str), "ANTHROPIC_API_KEY environment variable must be set"

    # Example 1: Simple text request
    simple_request = {
        "model": "anthropic/claude-3-5-sonnet-20241022",
        "input": "Tell me a three sentence bedtime story about a unicorn.",
        "env": {"ANTHROPIC_API_KEY": anthropic_api_key},
    }

    # Example 2: Multi-modal request with image
    multimodal_request = {
        "model": "anthropic/claude-3-5-sonnet-20241022",
        "input": [
            {
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "what is in this image?"},
                    {
                        "type": "input_image",
                        "image_url": "https://upload.wikimedia.org/wikipedia/commons/thumb/d/dd/Gfp-wisconsin-madison-the-nature-boardwalk.jpg/2560px-Gfp-wisconsin-madison-the-nature-boardwalk.jpg",
                    },
                ],
            }
        ],
        "env": {"ANTHROPIC_API_KEY": anthropic_api_key},
    }

    # Example 3: Request with custom agent and computer kwargs
    custom_request = {
        "model": "anthropic/claude-3-5-sonnet-20241022",
        "input": "Take a screenshot and tell me what you see",
        "env": {"ANTHROPIC_API_KEY": anthropic_api_key},
    }

    # Test requests
    base_url = "https://m-linux-96lcxd2c2k.containers.cloud.trycua.com:8443"
    # base_url = "http://localhost:8000"
    api_key = os.getenv("CUA_API_KEY")
    assert isinstance(api_key, str), "CUA_API_KEY environment variable must be set"

    async with aiohttp.ClientSession() as session:
        for i, request_data in enumerate(
            [
                simple_request,
                # multimodal_request,
                custom_request,
            ],
            1,
        ):
            print(f"\n--- Test {i} ---")
            print(f"Request: {json.dumps(request_data, indent=2)}")

            try:
                print(f"Sending request to {base_url}/responses")
                async with session.post(
                    f"{base_url}/responses",
                    json=request_data,
                    headers={"Content-Type": "application/json", "X-API-Key": api_key},
                ) as response:
                    result = await response.json()
                    print(f"Status: {response.status}")
                    print(f"Response: {json.dumps(result, indent=2)}")

            except Exception as e:
                print(f"Error: {e}")


def curl_examples():
    """Print curl command examples."""

    print("=== CURL Examples ===\n")

    print("1. Simple text request:")
    print(
        """curl http://localhost:8000/responses \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "anthropic/claude-3-5-sonnet-20241022",
    "input": "Tell me a three sentence bedtime story about a unicorn."
  }'"""
    )

    print("\n2. Multi-modal request with image:")
    print(
        """curl http://localhost:8000/responses \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "anthropic/claude-3-5-sonnet-20241022",
    "input": [
      {
        "role": "user",
        "content": [
          {"type": "input_text", "text": "what is in this image?"},
          {
            "type": "input_image",
            "image_url": "https://upload.wikimedia.org/wikipedia/commons/thumb/d/dd/Gfp-wisconsin-madison-the-nature-boardwalk.jpg/2560px-Gfp-wisconsin-madison-the-nature-boardwalk.jpg"
          }
        ]
      }
    ]
  }'"""
    )

    print("\n3. Request with custom configuration:")
    print(
        """curl http://localhost:8000/responses \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "anthropic/claude-3-5-sonnet-20241022",
    "input": "Take a screenshot and tell me what you see",
    "agent_kwargs": {
      "save_trajectory": true,
      "verbosity": 20
    },
    "computer_kwargs": {
      "os_type": "linux",
      "provider_type": "cloud"
    }
  }'"""
    )


async def test_p2p_client():
    """Example P2P client using peerjs-python."""
    try:
        from aiortc import RTCConfiguration, RTCIceServer
        from peerjs import ConnectionEventType, Peer, PeerOptions

        # Set up client peer
        options = PeerOptions(
            host="0.peerjs.com",
            port=443,
            secure=True,
            config=RTCConfiguration(iceServers=[RTCIceServer(urls="stun:stun.l.google.com:19302")]),
        )

        client_peer = Peer(id="test-client", peer_options=options)
        await client_peer.start()

        # Connect to proxy server
        connection = client_peer.connect("computer-agent-proxy")

        @connection.on(ConnectionEventType.Open)
        async def connection_open():
            print("Connected to proxy server")

            # Send a test request
            request = {
                "model": "anthropic/claude-3-5-sonnet-20241022",
                "input": "Hello from P2P client!",
            }
            await connection.send(json.dumps(request))

        @connection.on(ConnectionEventType.Data)
        async def connection_data(data):
            print(f"Received response: {data}")
            await client_peer.destroy()

        # Wait for connection
        await asyncio.sleep(10)

    except ImportError:
        print("P2P dependencies not available. Install peerjs-python for P2P testing.")
    except Exception as e:
        print(f"P2P test error: {e}")


if __name__ == "__main__":
    import sys

    if len(sys.argv) > 1 and sys.argv[1] == "curl":
        curl_examples()
    elif len(sys.argv) > 1 and sys.argv[1] == "p2p":
        asyncio.run(test_p2p_client())
    else:
        asyncio.run(test_http_endpoint())
