# CUA Agent Client Examples

This directory contains examples demonstrating how to use the `@trycua/agent` client library.

## Browser Example

### `browser-example.html`

A simple HTML page that demonstrates using the CUA Agent Client in a browser environment.

**Features:**

- Connect to HTTP/HTTPS or P2P (peer://) agent proxies
- Send text messages to any supported model
- View responses in real-time
- Health check functionality
- Clear, simple interface with no external dependencies

**Usage:**

1. **Build the library first:**

   ```bash
   cd ../
   pnpm build
   ```

2. **Start a local web server** (required for ES modules):

   ```bash
   # Option 1: Using Python
   python -m http.server 8080

   # Option 2: Using Node.js (if you have http-server installed)
   npx http-server -p 8080

   # Option 3: Using any other local server
   ```

3. **Open in browser:**
   Navigate to `http://localhost:8080/examples/playground-example.html`

4. **Configure and test:**
   - Enter an agent URL (e.g., `https://localhost:8000` or `peer://some-peer-id`)
   - Enter a model name (e.g., `anthropic/claude-3-5-sonnet-20241022`)
   - Type a message and click "Send Message" or press Enter
   - View the response in the output textarea

**Supported URLs:**

- **HTTP/HTTPS**: `https://localhost:8000`, `http://my-agent-server.com:8080`
- **Peer-to-Peer**: `peer://computer-agent-proxy`, `peer://any-peer-id`

**Example Models:**

- `anthropic/claude-3-5-sonnet-20241022`
- `openai/gpt-4`
- `huggingface-local/microsoft/UI-TARS-7B`

**Note:** Make sure you have a CUA agent proxy server running at the specified URL before testing.

## Running Agent Proxy Server

To test the examples, you'll need a CUA agent proxy server running:

```bash
# HTTP server (default port 8000)
python -m agent.proxy.cli

# P2P server
python -m agent.proxy.cli --mode p2p

# Both HTTP and P2P
python -m agent.proxy.cli --mode both
```
