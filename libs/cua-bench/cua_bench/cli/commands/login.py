"""Login command - Authenticate with CUA Cloud to get API token."""

import http.server
import json
import os
import socket
import threading
import urllib.parse
import webbrowser
from pathlib import Path

RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"

# Default configuration
DEFAULT_AUTH_URL = "https://cua.ai/cli-auth"
CONFIG_DIR = Path.home() / ".config" / "cua-bench"
TOKEN_FILE = CONFIG_DIR / "token.json"


def get_free_port():
    """Find a free port to use for the callback server."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("", 0))
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        return s.getsockname()[1]


def save_token(token: str, workspace_slug: str = None):
    """Save the authentication token to the config file."""
    CONFIG_DIR.mkdir(parents=True, exist_ok=True)

    data = {"token": token}
    if workspace_slug:
        data["workspace_slug"] = workspace_slug

    with open(TOKEN_FILE, "w") as f:
        json.dump(data, f, indent=2)

    # Set restrictive permissions
    os.chmod(TOKEN_FILE, 0o600)


def load_token() -> dict | None:
    """Load the authentication token from the config file."""
    if not TOKEN_FILE.exists():
        return None

    try:
        with open(TOKEN_FILE, "r") as f:
            return json.load(f)
    except (json.JSONDecodeError, IOError):
        return None


def execute(args):
    """Execute the login command."""
    # Check if already logged in
    existing = load_token()
    if existing and hasattr(args, "force") and not args.force:
        print(f"{YELLOW}Already logged in.{RESET}")
        print(f"{GREY}Use {RESET}{CYAN}cb login --force{RESET}{GREY} to re-authenticate.{RESET}")
        return 0

    # Get the auth URL
    auth_base_url = getattr(args, "auth_url", None) or os.getenv("CUA_AUTH_URL", DEFAULT_AUTH_URL)

    # Find a free port for the callback server
    port = get_free_port()
    callback_url = f"http://127.0.0.1:{port}/callback"

    # Build the auth URL with callback
    auth_url = f"{auth_base_url}?callback_url={urllib.parse.quote(callback_url)}"

    print(f"{CYAN}Opening browser for authentication...{RESET}")
    print(f"{GREY}If browser doesn't open, visit:{RESET}")
    print(f"  {auth_url}")
    print()

    # Store the received token
    received_token = {"token": None}
    server_done = threading.Event()

    class CallbackHandler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            parsed = urllib.parse.urlparse(self.path)
            if parsed.path == "/callback":
                query = urllib.parse.parse_qs(parsed.query)
                token = query.get("token", [None])[0]

                if token:
                    received_token["token"] = token
                    self.send_response(200)
                    self.send_header("Content-type", "text/html")
                    self.end_headers()
                    self.wfile.write(
                        b"""
                        <html>
                        <head>
                            <title>CUA CLI - Authenticated</title>
                            <style>
                                body { font-family: system-ui; text-align: center; padding: 50px; }
                                .success { color: #10b981; font-size: 24px; }
                            </style>
                        </head>
                        <body>
                            <div class="success">&#10003; Authentication successful!</div>
                            <p>You can close this window and return to the terminal.</p>
                        </body>
                        </html>
                    """
                    )
                else:
                    self.send_response(400)
                    self.send_header("Content-type", "text/html")
                    self.end_headers()
                    self.wfile.write(
                        b"""
                        <html>
                        <head><title>CUA CLI - Error</title></head>
                        <body>
                            <h1>Authentication failed</h1>
                            <p>No token received. Please try again.</p>
                        </body>
                        </html>
                    """
                    )

                # Signal that we're done
                server_done.set()
            else:
                self.send_response(404)
                self.end_headers()

        def log_message(self, format, *args):
            # Suppress HTTP server logs
            pass

    # Start the callback server
    server = http.server.HTTPServer(("127.0.0.1", port), CallbackHandler)
    server.timeout = 300  # 5 minute timeout

    def serve():
        while not server_done.is_set():
            server.handle_request()

    server_thread = threading.Thread(target=serve)
    server_thread.daemon = True
    server_thread.start()

    # Open the browser
    try:
        webbrowser.open(auth_url)
    except Exception as e:
        print(f"{YELLOW}Could not open browser: {e}{RESET}")
        print(f"{GREY}Please open the URL manually.{RESET}")

    print(f"{GREY}Waiting for authentication...{RESET}")

    # Wait for the callback
    server_done.wait(timeout=300)

    if received_token["token"]:
        # Save the token
        save_token(received_token["token"])
        print(f"\n{GREEN}✓ Successfully authenticated!{RESET}")
        print(f"{GREY}Token saved to: {TOKEN_FILE}{RESET}")
        print()
        print(f"{GREY}You can now use:{RESET}")
        print(f"  {CYAN}cb image upload <image-name>{RESET}  - Push an image to the registry")
        return 0
    else:
        print(f"\n{RED}✗ Authentication timed out or failed.{RESET}")
        print(f"{GREY}Please try again with:{RESET} {CYAN}cb login{RESET}")
        return 1
