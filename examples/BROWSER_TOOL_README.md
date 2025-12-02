# Browser Tool

Browser automation tool that allows agents to control a Firefox browser programmatically via Playwright while keeping it visible on the XFCE desktop.

## Quick Start

### Using Docker (Recommended)

```bash
# Build and run the container
cd libs/xfce
docker build -t cua-xfce .
docker run -d --name cua-xfce-test \
  -p 8000:8000 -p 5901:5901 -p 6901:6901 \
  -e DISPLAY=:1 \
  cua-xfce

# View desktop: http://localhost:6901
# Test the browser tool
python examples/browser_tool_example.py
```

### Local Testing

```bash
# Install dependencies
pip install playwright
playwright install --with-deps firefox

# Start server
python -m computer_server --port 8000

# Run test (in another terminal)
python examples/browser_tool_example.py
```

## Features

- **Visible Browser**: Runs in non-headless mode so visual agents can see it
- **Auto-Recovery**: Automatically reopens browser if closed manually
- **Persistent Context**: Maintains cookies and sessions across commands
- **Fara/Magentic-One Interface**: Compatible with Microsoft agent interfaces

## API Endpoint

The browser tool is accessible via the `/playwright_exec` endpoint:

```bash
curl -X POST http://localhost:8000/playwright_exec \
  -H "Content-Type: application/json" \
  -d '{"command": "visit_url", "params": {"url": "https://www.example.com"}}'
```

## Available Commands

- `visit_url(url)` - Navigate to a URL
- `click(x, y)` - Click at coordinates
- `type(text)` - Type text into focused element
- `scroll(delta_x, delta_y)` - Scroll the page
- `web_search(query)` - Navigate to Google search

## Troubleshooting

**Browser closes unexpectedly**: The tool automatically reopens the browser on the next command.

**Connection errors**: Make sure the server is running (`curl http://localhost:8000/status`).

**Playwright not found**: Install with `pip install playwright && playwright install --with-deps firefox`.

