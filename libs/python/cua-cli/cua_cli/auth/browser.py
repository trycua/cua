"""Browser-based OAuth authentication for CUA CLI."""

import asyncio
import os
import platform
import subprocess
import urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
from threading import Thread
from typing import Optional

# Default URLs
DEFAULT_WEBSITE_URL = "https://cua.ai"

# Timeout for browser authentication (2 minutes)
AUTH_TIMEOUT_SECONDS = 120


class CallbackHandler(BaseHTTPRequestHandler):
    """HTTP request handler for OAuth callback."""

    token: Optional[str] = None
    error: Optional[str] = None

    def do_GET(self) -> None:
        """Handle GET request from OAuth callback."""
        parsed = urllib.parse.urlparse(self.path)
        params = urllib.parse.parse_qs(parsed.query)

        if "token" in params:
            CallbackHandler.token = params["token"][0]
            self._send_response(
                200,
                "<html><body><h1>Authentication successful!</h1>"
                "<p>You can close this window and return to the terminal.</p></body></html>",
            )
        elif "error" in params:
            CallbackHandler.error = params.get("error_description", params["error"])[0]
            self._send_response(
                400,
                f"<html><body><h1>Authentication failed</h1>"
                f"<p>{CallbackHandler.error}</p></body></html>",
            )
        else:
            self._send_response(
                400,
                "<html><body><h1>Invalid callback</h1>"
                "<p>Missing token parameter.</p></body></html>",
            )

    def _send_response(self, status: int, body: str) -> None:
        """Send an HTTP response."""
        self.send_response(status)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        self.wfile.write(body.encode())

    def log_message(self, format: str, *args) -> None:
        """Suppress default logging."""
        pass


def open_browser(url: str) -> bool:
    """Open a URL in the default browser.

    Args:
        url: The URL to open

    Returns:
        True if the browser was opened successfully
    """
    system = platform.system()

    try:
        if system == "Darwin":
            subprocess.run(["open", url], check=True)
        elif system == "Windows":
            subprocess.run(["cmd", "/c", "start", url], check=True, shell=True)
        else:  # Linux and others
            subprocess.run(["xdg-open", url], check=True)
        return True
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


async def authenticate_via_browser(
    website_url: Optional[str] = None,
) -> str:
    """Authenticate via browser OAuth flow.

    Starts a local HTTP server to receive the callback, opens the browser
    to the authentication page, and waits for the token.

    Args:
        website_url: Base URL for the authentication page. Defaults to CUA_WEBSITE_URL
                    env var or https://cua.ai

    Returns:
        The API token

    Raises:
        TimeoutError: If authentication times out
        RuntimeError: If authentication fails
    """
    website_url = website_url or os.environ.get("CUA_WEBSITE_URL", DEFAULT_WEBSITE_URL)

    # Reset handler state
    CallbackHandler.token = None
    CallbackHandler.error = None

    # Start local server on dynamic port
    server = HTTPServer(("localhost", 0), CallbackHandler)
    port = server.server_address[1]
    callback_url = f"http://localhost:{port}"

    # Build auth URL
    encoded_callback = urllib.parse.quote(callback_url, safe="")
    auth_url = f"{website_url}/cli-auth?callback_url={encoded_callback}"

    # Start server in background thread
    server_thread = Thread(target=server.handle_request, daemon=True)
    server_thread.start()

    # Open browser
    print("Opening browser for authentication...")
    if not open_browser(auth_url):
        print("Could not open browser automatically.")
        print(f"Please visit: {auth_url}")

    # Wait for callback with timeout
    try:
        for _ in range(AUTH_TIMEOUT_SECONDS):
            if CallbackHandler.token or CallbackHandler.error:
                break
            await asyncio.sleep(1)
        else:
            raise TimeoutError("Authentication timed out. Please try again.")

        if CallbackHandler.error:
            raise RuntimeError(f"Authentication failed: {CallbackHandler.error}")

        if not CallbackHandler.token:
            raise RuntimeError("Authentication failed: No token received")

        return CallbackHandler.token

    finally:
        server.server_close()
