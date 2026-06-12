"""
Command-line interface for the Computer API server.
"""

import argparse
import logging
import os
import sys
from typing import List, Optional

logger = logging.getLogger(__name__)


def parse_args(args: Optional[List[str]] = None) -> argparse.Namespace:
    """Parse command-line arguments."""
    parser = argparse.ArgumentParser(description="Start the Computer API server")
    parser.add_argument(
        "--width",
        type=int,
        help="Target width for screenshots (coordinates will be scaled accordingly)",
    )
    parser.add_argument(
        "--height",
        type=int,
        help="Target height for screenshots (coordinates will be scaled accordingly)",
    )
    parser.add_argument(
        "--detect-resolution",
        action="store_true",
        help="Auto-detect and log the actual screen resolution at startup",
    )
    parser.add_argument(
        "--host", default="0.0.0.0", help="Host to bind the server to (default: 0.0.0.0)"
    )
    parser.add_argument(
        "--port", type=int, default=8000, help="Port to bind the server to (default: 8000)"
    )
    parser.add_argument(
        "--log-level",
        choices=["debug", "info", "warning", "error", "critical"],
        default="info",
        help="Logging level (default: info)",
    )
    parser.add_argument(
        "--ssl-keyfile",
        type=str,
        help="Path to SSL private key file (enables HTTPS)",
    )
    parser.add_argument(
        "--ssl-certfile",
        type=str,
        help="Path to SSL certificate file (enables HTTPS)",
    )
    # Backend selection
    parser.add_argument(
        "--backend",
        choices=["native", "vnc"],
        default="native",
        help="Handler backend: 'native' uses OS-specific handlers, 'vnc' uses VNC (default: native)",
    )

    # VNC backend options
    parser.add_argument(
        "--vnc-host",
        type=str,
        help="VNC server host (required when --backend=vnc)",
    )
    parser.add_argument(
        "--vnc-port",
        type=int,
        default=5900,
        help="VNC server port (default: 5900)",
    )
    parser.add_argument(
        "--vnc-password",
        type=str,
        default="",
        help="VNC server password",
    )

    return parser.parse_args(args)


def main() -> None:
    """Main entry point for the CLI."""
    args = parse_args()

    # Configure logging
    logging.basicConfig(
        level=getattr(logging, args.log_level.upper()),
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
        stream=sys.stderr,  # Use stderr for MCP compatibility
    )

    # Set backend env vars from CLI args before Server import triggers handler creation
    if args.backend == "vnc" or args.vnc_host:
        if not args.vnc_host and not os.environ.get("CUA_VNC_HOST"):
            parser_err = "--vnc-host is required when using --backend=vnc"
            logger.error(parser_err)
            sys.exit(1)
        os.environ["CUA_BACKEND"] = "vnc"
        if args.vnc_host:
            os.environ["CUA_VNC_HOST"] = args.vnc_host
        # Only override env vars from CLI args when explicitly provided
        if args.vnc_port != 5900 or "CUA_VNC_PORT" not in os.environ:
            os.environ["CUA_VNC_PORT"] = str(args.vnc_port)
        if args.vnc_password:
            os.environ["CUA_VNC_PASSWORD"] = args.vnc_password
        vnc_host = args.vnc_host or os.environ.get("CUA_VNC_HOST")
        logger.info(f"VNC backend enabled → {vnc_host}:{args.vnc_port}")

    # Create and start the server
    logger.info(f"Starting Cua Computer API server on {args.host}:{args.port}...")
    logger.info("HTTP API available at /ws, /cmd, /status endpoints")
    logger.info("MCP server available at /mcp endpoint (if fastmcp installed)")

    # Handle SSL configuration
    ssl_args = {}
    if args.ssl_keyfile and args.ssl_certfile:
        ssl_args = {
            "ssl_keyfile": args.ssl_keyfile,
            "ssl_certfile": args.ssl_certfile,
        }
        logger.info("HTTPS mode enabled with SSL certificates")
    elif args.ssl_keyfile or args.ssl_certfile:
        logger.warning(
            "Both --ssl-keyfile and --ssl-certfile are required for HTTPS. Running in HTTP mode."
        )
    else:
        logger.info("HTTP mode (no SSL certificates provided)")

    # Import Server lazily so env vars (e.g. CUA_VNC_HOST) are set before
    # the module-level handler factory runs in main.py.
    from .server import Server

    server = Server(host=args.host, port=args.port, log_level=args.log_level, **ssl_args)

    try:
        server.start()
    except KeyboardInterrupt:
        logger.info("Server stopped by user")
        sys.exit(0)
    except Exception as e:
        logger.error(f"Error starting server: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
