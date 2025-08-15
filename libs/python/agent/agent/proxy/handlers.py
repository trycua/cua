"""
Request handlers for the proxy endpoints.
"""

import asyncio
import json
import logging
import os
from typing import Dict, Any, List, Union, Optional

from ..agent import ComputerAgent
from computer import Computer

logger = logging.getLogger(__name__)


class ResponsesHandler:
    """Handler for /responses endpoint that processes agent requests."""
    
    def __init__(self):
        self.computer = None
        self.agent = None
    
    async def setup_computer(self, computer_kwargs: Optional[Dict[str, Any]] = None):
        """Set up computer instance with provided kwargs or defaults."""
        if self.computer is not None:
            return  # Already set up
            
        # Default computer configuration
        default_config = {
            "os_type": "linux",
            "provider_type": "cloud",
            "name": os.getenv("CUA_CONTAINER_NAME"),
            "api_key": os.getenv("CUA_API_KEY")
        }
        
        # Override with provided kwargs
        if computer_kwargs:
            default_config.update(computer_kwargs)
            
        self.computer = Computer(**default_config)
        await self.computer.__aenter__()
        logger.info(f"Computer set up with config: {default_config}")
    
    async def setup_agent(self, model: str, agent_kwargs: Optional[Dict[str, Any]] = None):
        """Set up agent instance with provided model and kwargs."""
        if self.computer is None:
            raise RuntimeError("Computer must be set up before agent")
            
        # Default agent configuration
        default_config = {
            "model": model,
            "tools": [self.computer]
        }
        
        # Override with provided kwargs
        if agent_kwargs:
            # Don't override tools unless explicitly provided
            if "tools" not in agent_kwargs:
                agent_kwargs["tools"] = [self.computer]
            default_config.update(agent_kwargs)
            
        self.agent = ComputerAgent(**default_config)
        logger.info(f"Agent set up with model: {model}")
    
    async def process_request(self, request_data: Dict[str, Any]) -> Dict[str, Any]:
        """
        Process a /responses request and return the result.
        
        Args:
            request_data: Dictionary containing model, input, and optional kwargs
            
        Returns:
            Dictionary with the agent's response
        """
        try:
            # Extract request parameters
            model = request_data.get("model")
            input_data = request_data.get("input")
            agent_kwargs = request_data.get("agent_kwargs", {})
            computer_kwargs = request_data.get("computer_kwargs", {})
            
            if not model:
                raise ValueError("Model is required")
            if not input_data:
                raise ValueError("Input is required")
            
            # Set up computer and agent
            await self.setup_computer(computer_kwargs)
            await self.setup_agent(model, agent_kwargs)
            
            # Convert input to messages format
            messages = self._convert_input_to_messages(input_data)
            
            # Run agent and get first result
            async for result in self.agent.run(messages):
                # Return the first result and break
                return {
                    "success": True,
                    "result": result,
                    "model": model
                }
                
            # If no results were yielded
            return {
                "success": False,
                "error": "No results from agent",
                "model": model
            }
            
        except Exception as e:
            logger.error(f"Error processing request: {e}")
            return {
                "success": False,
                "error": str(e),
                "model": request_data.get("model", "unknown")
            }
    
    def _convert_input_to_messages(self, input_data: Union[str, List[Dict[str, Any]]]) -> List[Dict[str, Any]]:
        """Convert input data to messages format."""
        if isinstance(input_data, str):
            # Simple string input
            return [{"role": "user", "content": input_data}]
        elif isinstance(input_data, list):
            # Already in messages format
            messages = []
            for msg in input_data:
                # Convert content array format if needed
                if isinstance(msg.get("content"), list):
                    content_parts = []
                    for part in msg["content"]:
                        if part.get("type") == "input_text":
                            content_parts.append({"type": "text", "text": part["text"]})
                        elif part.get("type") == "input_image":
                            content_parts.append({
                                "type": "image_url",
                                "image_url": {"url": part["image_url"]}
                            })
                        else:
                            content_parts.append(part)
                    messages.append({
                        "role": msg["role"],
                        "content": content_parts
                    })
                else:
                    messages.append(msg)
            return messages
        else:
            raise ValueError("Input must be string or list of messages")
    
    async def cleanup(self):
        """Clean up resources."""
        if self.computer:
            try:
                await self.computer.__aexit__(None, None, None)
            except Exception as e:
                logger.error(f"Error cleaning up computer: {e}")
            finally:
                self.computer = None
        self.agent = None
