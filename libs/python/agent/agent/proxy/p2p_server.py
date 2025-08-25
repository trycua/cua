"""
P2P server implementation using peerjs-python for WebRTC connections.
"""

import asyncio
import json
import logging
import uuid
from typing import Dict, Any, Optional

logger = logging.getLogger(__name__)


class P2PServer:
    """P2P server using peerjs-python for WebRTC connections."""
    
    def __init__(self, handler, peer_id: Optional[str] = None, signaling_server: Optional[Dict[str, Any]] = None):
        self.handler = handler
        self.peer_id = peer_id or uuid.uuid4().hex
        self.signaling_server = signaling_server or {
            "host": "0.peerjs.com",
            "port": 443,
            "secure": True
        }
        self.peer = None
    
    async def start(self):
        """Start P2P server with WebRTC connections."""
        try:
            from peerjs_py.peer import Peer, PeerOptions
            from peerjs_py.enums import PeerEventType, ConnectionEventType
            
            # Set up peer options
            ice_servers = [
                {"urls": "stun:stun.l.google.com:19302"},
                {"urls": "stun:stun1.l.google.com:19302"}
            ]
            
            # Create peer with PeerOptions (config should be a dict, not RTCConfiguration)
            peer_options = PeerOptions(
                host=self.signaling_server["host"],
                port=self.signaling_server["port"],
                secure=self.signaling_server["secure"],
                config={
                    "iceServers": ice_servers
                }
            )
            
            # Create peer
            self.peer = Peer(id=self.peer_id, options=peer_options)
            await self.peer.start()
            # logger.info(f"P2P peer started with ID: {self.peer_id}")
            print(f"Agent proxy started at peer://{self.peer_id}")
            
            # Set up connection handlers using string event names
            @self.peer.on('connection')
            async def peer_connection(peer_connection):
                logger.info(f"Remote peer {peer_connection.peer} trying to establish connection")
                await self._setup_connection_handlers(peer_connection)
            
            @self.peer.on('error')
            async def peer_error(error):
                logger.error(f"Peer error: {error}")
            
            # Keep the server running
            while True:
                await asyncio.sleep(1)
                
        except ImportError as e:
            logger.error(f"P2P dependencies not available: {e}")
            logger.error("Install peerjs-python: pip install peerjs-python")
            raise
        except Exception as e:
            logger.error(f"Error starting P2P server: {e}")
            raise
    
    async def _setup_connection_handlers(self, peer_connection):
        """Set up handlers for a peer connection."""
        try:
            # Use string event names instead of enum types
            
            @peer_connection.on('open')
            async def connection_open():
                logger.info(f"Connection opened with peer {peer_connection.peer}")
                
                # Send welcome message
                welcome_msg = {
                    "type": "welcome",
                    "message": "Connected to ComputerAgent Proxy",
                    "endpoints": ["/responses"]
                }
                await peer_connection.send(json.dumps(welcome_msg))
            
            @peer_connection.on('data')
            async def connection_data(data):
                logger.debug(f"Data received from peer {peer_connection.peer}: {data}")
                
                try:
                    # Parse the incoming data
                    if isinstance(data, str):
                        request_data = json.loads(data)
                    else:
                        request_data = data
                    
                    # Check if it's an HTTP-like request
                    if self._is_http_request(request_data):
                        response = await self._handle_http_request(request_data)
                        await peer_connection.send(json.dumps(response))
                    else:
                        # Direct API request
                        result = await self.handler.process_request(request_data)
                        await peer_connection.send(json.dumps(result))
                        
                except json.JSONDecodeError:
                    error_response = {
                        "success": False,
                        "error": "Invalid JSON in request"
                    }
                    await peer_connection.send(json.dumps(error_response))
                except Exception as e:
                    logger.error(f"Error processing P2P request: {e}")
                    error_response = {
                        "success": False,
                        "error": str(e)
                    }
                    await peer_connection.send(json.dumps(error_response))
            
            @peer_connection.on('close')
            async def connection_close():
                logger.info(f"Connection closed with peer {peer_connection.peer}")
            
            @peer_connection.on('error')
            async def connection_error(error):
                logger.error(f"Connection error with peer {peer_connection.peer}: {error}")
                
        except Exception as e:
            logger.error(f"Error setting up connection handlers: {e}")
    
    def _is_http_request(self, data: Dict[str, Any]) -> bool:
        """Check if the data looks like an HTTP request."""
        return (
            isinstance(data, dict) and
            "method" in data and
            "path" in data and
            data.get("method") == "POST" and
            data.get("path") == "/responses"
        )
    
    async def _handle_http_request(self, request_data: Dict[str, Any]) -> Dict[str, Any]:
        """Handle HTTP-like request over P2P."""
        try:
            method = request_data.get("method")
            path = request_data.get("path")
            body = request_data.get("body", {})
            
            if method == "POST" and path == "/responses":
                # Process the request body
                result = await self.handler.process_request(body)
                return {
                    "status": 200,
                    "headers": {"Content-Type": "application/json"},
                    "body": result
                }
            else:
                return {
                    "status": 404,
                    "headers": {"Content-Type": "application/json"},
                    "body": {"success": False, "error": "Endpoint not found"}
                }
                
        except Exception as e:
            logger.error(f"Error handling HTTP request: {e}")
            return {
                "status": 500,
                "headers": {"Content-Type": "application/json"},
                "body": {"success": False, "error": str(e)}
            }
    
    async def stop(self):
        """Stop the P2P server."""
        if self.peer:
            try:
                await self.peer.destroy()
                logger.info("P2P peer stopped")
            except Exception as e:
                logger.error(f"Error stopping P2P peer: {e}")
            finally:
                self.peer = None
