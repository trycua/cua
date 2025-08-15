"""
Proxy server implementation supporting both HTTP and P2P connections.
"""

import asyncio
import json
import logging
from typing import Dict, Any, Optional
from starlette.applications import Starlette
from starlette.routing import Route
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.middleware import Middleware
from starlette.middleware.cors import CORSMiddleware
import uvicorn

from .handlers import ResponsesHandler

logger = logging.getLogger(__name__)


class ProxyServer:
    """Proxy server that can serve over HTTP and P2P."""
    
    def __init__(self, host: str = "0.0.0.0", port: int = 8000):
        self.host = host
        self.port = port
        self.handler = ResponsesHandler()
        self.app = self._create_app()
    
    def _create_app(self) -> Starlette:
        """Create Starlette application with routes."""
        
        async def responses_endpoint(request: Request) -> JSONResponse:
            """Handle POST /responses requests."""
            try:
                # Parse JSON body
                body = await request.body()
                request_data = json.loads(body.decode('utf-8'))
                
                # Process the request
                result = await self.handler.process_request(request_data)
                
                return JSONResponse(result)
                
            except json.JSONDecodeError:
                return JSONResponse({
                    "success": False,
                    "error": "Invalid JSON in request body"
                }, status_code=400)
            except Exception as e:
                logger.error(f"Error in responses endpoint: {e}")
                return JSONResponse({
                    "success": False,
                    "error": str(e)
                }, status_code=500)
        
        async def health_endpoint(request: Request) -> JSONResponse:
            """Health check endpoint."""
            return JSONResponse({"status": "healthy"})
        
        routes = [
            Route("/responses", responses_endpoint, methods=["POST"]),
            Route("/health", health_endpoint, methods=["GET"]),
        ]
        
        middleware = [
            Middleware(CORSMiddleware, 
                      allow_origins=["*"],
                      allow_credentials=True,
                      allow_methods=["*"],
                      allow_headers=["*"])
        ]
        
        return Starlette(routes=routes, middleware=middleware)
    
    async def start_http(self):
        """Start HTTP server."""
        logger.info(f"Starting HTTP server on {self.host}:{self.port}")
        config = uvicorn.Config(
            self.app,
            host=self.host,
            port=self.port,
            log_level="info"
        )
        server = uvicorn.Server(config)
        await server.serve()
    
    async def start_p2p(self, peer_id: Optional[str] = None, signaling_server: Optional[Dict[str, Any]] = None):
        """Start P2P server using peerjs-python."""
        try:
            from .p2p_server import P2PServer
            
            p2p_server = P2PServer(
                handler=self.handler,
                peer_id=peer_id,
                signaling_server=signaling_server
            )
            await p2p_server.start()
            
        except ImportError:
            logger.error("P2P dependencies not available. Install peerjs-python for P2P support.")
            raise
    
    async def start(self, mode: str = "http", **kwargs):
        """
        Start the server in specified mode.
        
        Args:
            mode: "http", "p2p", or "both"
            **kwargs: Additional arguments for specific modes
        """
        if mode == "http":
            await self.start_http()
        elif mode == "p2p":
            await self.start_p2p(**kwargs)
        elif mode == "both":
            # Start both HTTP and P2P servers concurrently
            tasks = [
                asyncio.create_task(self.start_http()),
                asyncio.create_task(self.start_p2p(**kwargs))
            ]
            await asyncio.gather(*tasks)
        else:
            raise ValueError(f"Invalid mode: {mode}. Must be 'http', 'p2p', or 'both'")
    
    async def cleanup(self):
        """Clean up resources."""
        await self.handler.cleanup()


async def main():
    """Main entry point for running the proxy server."""
    import argparse
    
    parser = argparse.ArgumentParser(description="ComputerAgent Proxy Server")
    parser.add_argument("--host", default="0.0.0.0", help="Host to bind to")
    parser.add_argument("--port", type=int, default=8000, help="Port to bind to")
    parser.add_argument("--mode", choices=["http", "p2p", "both"], default="http", 
                       help="Server mode")
    parser.add_argument("--peer-id", help="Peer ID for P2P mode")
    parser.add_argument("--signaling-host", default="0.peerjs.com", help="Signaling server host")
    parser.add_argument("--signaling-port", type=int, default=443, help="Signaling server port")
    parser.add_argument("--signaling-secure", action="store_true", help="Use secure signaling")
    
    args = parser.parse_args()
    
    # Set up logging
    logging.basicConfig(level=logging.INFO)
    
    # Create server
    server = ProxyServer(host=args.host, port=args.port)
    
    # Prepare P2P kwargs if needed
    p2p_kwargs = {}
    if args.mode in ["p2p", "both"]:
        p2p_kwargs = {
            "peer_id": args.peer_id,
            "signaling_server": {
                "host": args.signaling_host,
                "port": args.signaling_port,
                "secure": args.signaling_secure
            }
        }
    
    try:
        await server.start(mode=args.mode, **p2p_kwargs)
    except KeyboardInterrupt:
        logger.info("Shutting down server...")
    finally:
        await server.cleanup()


if __name__ == "__main__":
    asyncio.run(main())
