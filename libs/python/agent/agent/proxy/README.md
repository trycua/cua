# ComputerAgent Proxy

A proxy server that exposes ComputerAgent functionality over HTTP and P2P (WebRTC) connections. This allows remote clients to interact with ComputerAgent instances through a simple REST-like API.

## Features

- **HTTP Server**: Standard HTTP REST API using Starlette
- **P2P Server**: WebRTC-based peer-to-peer connections using peerjs-python
- **No Pydantic**: Uses plain dictionaries for request/response handling
- **OpenAI-compatible API**: Similar request format to OpenAI's API
- **Multi-modal Support**: Handles text and image inputs
- **Configurable**: Supports custom agent and computer configurations

## Installation

The proxy requires additional dependencies:

```bash
# For HTTP server
pip install starlette uvicorn

# For P2P server (optional)
pip install peerjs-python aiortc
```

## Usage

### Starting the Server

```bash
# HTTP server only (default)
python -m agent.proxy.cli

# Custom host/port
python -m agent.proxy.cli --host 0.0.0.0 --port 8080

# P2P server only
python -m agent.proxy.cli --mode p2p --peer-id my-agent-proxy

# Both HTTP and P2P
python -m agent.proxy.cli --mode both --peer-id my-agent-proxy
```

### API Endpoints

#### POST /responses

Process a request using ComputerAgent and return the first result.

**Request Format:**
```json
{
  "model": "anthropic/claude-3-5-sonnet-20241022",
  "input": "Your instruction here",
  "agent_kwargs": {
    "save_trajectory": true,
    "verbosity": 20
  },
  "computer_kwargs": {
    "os_type": "linux",
    "provider_type": "cloud"
  }
}
```

**Multi-modal Request:**
```json
{
  "model": "anthropic/claude-3-5-sonnet-20241022",
  "input": [
    {
      "role": "user",
      "content": [
        {"type": "input_text", "text": "what is in this image?"},
        {
          "type": "input_image",
          "image_url": "https://example.com/image.jpg"
        }
      ]
    }
  ]
}
```

**Response Format:**
```json
{
  "success": true,
  "result": {
    // Agent response data
  },
  "model": "anthropic/claude-3-5-sonnet-20241022"
}
```

#### GET /health

Health check endpoint.

### cURL Examples

```bash
# Simple text request
curl http://localhost:8000/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "anthropic/claude-3-5-sonnet-20241022",
    "input": "Tell me a three sentence bedtime story about a unicorn."
  }'

# Multi-modal request
curl http://localhost:8000/responses \
  -H "Content-Type: application/json" \
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
  }'
```

### P2P Usage

The P2P server allows WebRTC connections for direct peer-to-peer communication:

```python
# Connect to P2P proxy
from peerjs import Peer, PeerOptions
import json

peer = Peer(id="client", peer_options=PeerOptions(host="localhost", port=9000))
await peer.start()

connection = peer.connect("computer-agent-proxy")

# Send request
request = {
    "model": "anthropic/claude-3-5-sonnet-20241022",
    "input": "Hello from P2P!"
}
await connection.send(json.dumps(request))
```

## Configuration

### Environment Variables

- `CUA_CONTAINER_NAME`: Default container name for cloud provider
- `CUA_API_KEY`: Default API key for cloud provider

### Request Parameters

- `model`: Model string (required) - e.g., "anthropic/claude-3-5-sonnet-20241022"
- `input`: String or message array (required)
- `agent_kwargs`: Optional agent configuration
- `computer_kwargs`: Optional computer configuration

### Agent Configuration (`agent_kwargs`)

Common options:
- `save_trajectory`: Boolean - Save conversation trajectory
- `verbosity`: Integer - Logging level (10=DEBUG, 20=INFO, etc.)
- `max_trajectory_budget`: Float - Budget limit for trajectory

### Computer Configuration (`computer_kwargs`)

Common options:
- `os_type`: String - "linux", "windows", "macos"
- `provider_type`: String - "cloud", "local", "docker"
- `name`: String - Instance name
- `api_key`: String - Provider API key

## Architecture

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   HTTP Client   │    │   P2P Client    │    │  Direct Usage   │
└─────────┬───────┘    └─────────┬───────┘    └─────────┬───────┘
          │                      │                      │
          ▼                      ▼                      ▼
┌─────────────────────────────────────────────────────────────────┐
│                    ProxyServer                                  │
├─────────────────────────────────────────────────────────────────┤
│                 ResponsesHandler                                │
├─────────────────────────────────────────────────────────────────┤
│              ComputerAgent + Computer                           │
└─────────────────────────────────────────────────────────────────┘
```

## Examples

See `examples.py` for complete usage examples:

```bash
# Run HTTP tests
python agent/proxy/examples.py

# Show curl examples
python agent/proxy/examples.py curl

# Test P2P (requires peerjs-python)
python agent/proxy/examples.py p2p
```

## Error Handling

The proxy returns structured error responses:

```json
{
  "success": false,
  "error": "Error description",
  "model": "model-used"
}
```

Common errors:
- Missing required parameters (`model`, `input`)
- Invalid JSON in request body
- Agent execution errors
- Computer setup failures

## Limitations

- Returns only the first result from agent.run() (as requested)
- P2P requires peerjs-python and signaling server
- Computer instances are created per request (not pooled)
- No authentication/authorization built-in
