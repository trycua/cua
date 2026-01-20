"""
Command-line interface for the Computer API server.
"""

import argparse
import asyncio
import logging
import os
import sys
import threading
from typing import List, Optional

from .server import Server

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
    parser.add_argument(
        "--watchdog",
        action="store_true",
        help="Enable watchdog monitoring (automatically enabled if CONTAINER_NAME env var is set)",
    )
    parser.add_argument(
        "--watchdog-interval",
        type=int,
        default=30,
        help="Watchdog ping interval in seconds (default: 30)",
    )
    parser.add_argument(
        "--no-restart",
        action="store_true",
        help="Disable automatic server restart in watchdog",
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

    # Auto-detect mode based on stdin:
    # - If stdin is a pipe (FIFO), we're being run as a subprocess with IPC (e.g., by Claude Code)
    #   → use MCP stdio mode for direct communication
    # - Otherwise (TTY, /dev/null, closed, regular file, etc.)
    #   → use HTTP server with MCP available at /mcp endpoint
    #
    # This distinction matters because:
    # - Claude Code connects stdin to a pipe for JSON-RPC communication
    # - LaunchAgents/systemd typically connect stdin to /dev/null (not a pipe)
    # - Interactive terminals are TTYs (not pipes)
    import stat
    try:
        stdin_mode = os.fstat(sys.stdin.fileno()).st_mode
        use_mcp_stdio = stat.S_ISFIFO(stdin_mode)  # True only if stdin is a pipe
    except (OSError, AttributeError):
        # stdin might be closed or invalid - use HTTP mode
        use_mcp_stdio = False

    if use_mcp_stdio:
        logger.info("Detected subprocess mode (stdin is pipe) - starting MCP stdio server...")
        try:
            from .mcp_server import run_mcp_server
            run_mcp_server(
                target_width=args.width,
                target_height=args.height,
                detect_resolution=args.detect_resolution,
            )
        except ImportError as e:
            logger.error(f"MCP stdio mode requires fastmcp package: {e}")
            logger.error("Install with: pip install 'cua-computer-server[mcp]'")
            sys.exit(1)
        except KeyboardInterrupt:
            logger.info("MCP server stopped by user")
            sys.exit(0)
        except Exception as e:
            logger.error(f"Error running MCP server: {e}")
            sys.exit(1)
        return

    # Check if watchdog should be enabled
    container_name = os.environ.get("CONTAINER_NAME")
    enable_watchdog = (args.watchdog or bool(container_name)) and not sys.platform.startswith("win")

    if container_name:
        logger.info(
            f"Container environment detected (CONTAINER_NAME={container_name}), enabling watchdog"
        )
    elif args.watchdog:
        logger.info("Watchdog explicitly enabled via --watchdog flag")

    # Start watchdog if enabled
    if enable_watchdog:
        logger.info(f"Starting watchdog monitoring with {args.watchdog_interval}s interval")

        def run_watchdog_thread():
            """Run watchdog in a separate thread."""
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)
            try:
                # Create CLI args dict for watchdog
                cli_args = {
                    "host": args.host,
                    "port": args.port,
                    "log_level": args.log_level,
                    "ssl_keyfile": args.ssl_keyfile,
                    "ssl_certfile": args.ssl_certfile,
                }

                # Create watchdog with restart settings
                from .watchdog import Watchdog

                watchdog = Watchdog(cli_args=cli_args, ping_interval=args.watchdog_interval)
                watchdog.restart_enabled = not args.no_restart

                loop.run_until_complete(watchdog.start_monitoring())
            except Exception as e:
                logger.error(f"Watchdog error: {e}")
            finally:
                loop.close()

        # Start watchdog in background thread
        watchdog_thread = threading.Thread(target=run_watchdog_thread, daemon=True, name="watchdog")
        watchdog_thread.start()

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
