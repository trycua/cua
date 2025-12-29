"""
Request handlers for the proxy endpoints.
"""

import asyncio
import json
import logging
import os
from contextlib import contextmanager
from typing import Any, Dict, List, Optional, Union

from computer import Computer

from ..agent import ComputerAgent

logger = logging.getLogger(__name__)


class ResponsesHandler:
    """Handler for /responses endpoint that processes agent requests."""

    def __init__(self):
        self.computer = None
        self.agent = None
        # Simple in-memory caches
        self._computer_cache: Dict[str, Any] = {}
        self._agent_cache: Dict[str, Any] = {}

    async def setup_computer_agent(
        self,
        model: str,
        agent_kwargs: Optional[Dict[str, Any]] = None,
        computer_kwargs: Optional[Dict[str, Any]] = None,
    ):
        """Set up (and cache) computer and agent instances.

        Caching keys:
        - Computer cache key: computer_kwargs
        - Agent cache key: {"model": model, **agent_kwargs}
        """
        agent_kwargs = agent_kwargs or {}
        computer_kwargs = computer_kwargs or {}

        def _stable_key(obj: Dict[str, Any]) -> str:
            try:
                return json.dumps(obj, sort_keys=True, separators=(",", ":"))
            except Exception:
                # Fallback: stringify non-serializable values
                safe_obj = {}
                for k, v in obj.items():
                    try:
                        json.dumps(v)
                        safe_obj[k] = v
                    except Exception:
                        safe_obj[k] = str(v)
                return json.dumps(safe_obj, sort_keys=True, separators=(",", ":"))

        # Determine if custom tools are supplied; if so, skip computer setup entirely
        has_custom_tools = bool(agent_kwargs.get("tools"))

        computer = None
        if not has_custom_tools:
            # ---------- Computer setup (with cache) ----------
            comp_key = _stable_key(computer_kwargs)

            computer = self._computer_cache.get(comp_key)
            if computer is None:
                # Default computer configuration
                default_c_config = {
                    "os_type": "linux",
                    "provider_type": "cloud",
                    "name": os.getenv("CUA_CONTAINER_NAME"),
                    "api_key": os.getenv("CUA_API_KEY"),
                }
                default_c_config.update(computer_kwargs)
                computer = Computer(**default_c_config)
                await computer.__aenter__()
                self._computer_cache[comp_key] = computer
                logger.info(
                    f"Computer created and cached with key={comp_key} config={default_c_config}"
                )
            else:
                logger.info(f"Reusing cached computer for key={comp_key}")

        # Bind current computer reference (None if custom tools supplied)
        self.computer = computer

        # ---------- Agent setup (with cache) ----------
        # Build agent cache key from {model} + agent_kwargs (excluding tools unless explicitly passed)
        agent_kwargs_for_key = dict(agent_kwargs)
        agent_key_payload = {"model": model, **agent_kwargs_for_key}
        agent_key = _stable_key(agent_key_payload)

        agent = self._agent_cache.get(agent_key)
        if agent is None:
            # Default agent configuration
            default_a_config: Dict[str, Any] = {"model": model}
            if not has_custom_tools:
                default_a_config["tools"] = [computer]
            # Apply user overrides, but keep tools unless user explicitly sets
            if agent_kwargs:
                if not has_custom_tools:
                    agent_kwargs.setdefault("tools", [computer])
                default_a_config.update(agent_kwargs)
            # JSON-derived kwargs may have loose types; ignore static arg typing here
            agent = ComputerAgent(**default_a_config)  # type: ignore[arg-type]
            self._agent_cache[agent_key] = agent
            logger.info(f"Agent created and cached with key={agent_key} model={model}")
        else:
            # Ensure cached agent uses the current computer tool (in case object differs)
            # Only update if tools not explicitly provided in agent_kwargs
            if not has_custom_tools:
                try:
                    agent.tools = [computer]
                except Exception:
                    pass
            logger.info(f"Reusing cached agent for key={agent_key}")

        # Bind current agent reference
        self.agent = agent

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
            env_overrides = request_data.get("env", {}) or {}

            if not model:
                raise ValueError("Model is required")
            if not input_data:
                raise ValueError("Input is required")

            # Apply env overrides for the duration of this request
            with self._env_overrides(env_overrides):
                # Set up (and possibly reuse) computer and agent via caches
                await self.setup_computer_agent(model, agent_kwargs, computer_kwargs)

                # Defensive: ensure agent is initialized for type checkers
                agent = self.agent
                if agent is None:
                    raise RuntimeError("Agent failed to initialize")

                # Convert input to messages format
                messages = self._convert_input_to_messages(input_data)

                # Run agent and get first result
                async for result in agent.run(messages):
                    # Return the first result and break
                    return {"success": True, "result": result, "model": model}

            # If no results were yielded
            return {"success": False, "error": "No results from agent", "model": model}

        except Exception as e:
            logger.error(f"Error processing request: {e}")
            return {
                "success": False,
                "error": str(e),
                "model": request_data.get("model", "unknown"),
            }

    def _convert_input_to_messages(
        self, input_data: Union[str, List[Dict[str, Any]]]
    ) -> List[Dict[str, Any]]:
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
                            content_parts.append(
                                {"type": "image_url", "image_url": {"url": part["image_url"]}}
                            )
                        else:
                            content_parts.append(part)
                    messages.append({"role": msg["role"], "content": content_parts})
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

    @staticmethod
    @contextmanager
    def _env_overrides(env: Dict[str, str]):
        """Temporarily apply environment variable overrides for the current process.
        Restores previous values after the context exits.

        Args:
            env: Mapping of env var names to override for this request.
        """
        if not env:
            # No-op context
            yield
            return

        original: Dict[str, Optional[str]] = {}
        try:
            for k, v in env.items():
                original[k] = os.environ.get(k)
                os.environ[k] = str(v)
            yield
        finally:
            for k, old in original.items():
                if old is None:
                    # Was not set before
                    os.environ.pop(k, None)
                else:
                    os.environ[k] = old
