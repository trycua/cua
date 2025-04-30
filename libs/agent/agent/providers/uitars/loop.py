"""UI-TARS-specific agent loop implementation."""

import logging
import asyncio
import re
import os
import json
import base64
import copy
from typing import Any, Dict, List, Optional, Tuple, AsyncGenerator, cast

from httpx import ConnectError, ReadTimeout

from ...core.base import BaseLoop
from ...core.messages import StandardMessageManager, ImageRetentionConfig
from ...core.types import AgentResponse, LLMProvider
from ...core.visualization import VisualizationHelper
from computer import Computer

from .utils import add_box_token, parse_actions, parse_action_parameters, to_agent_response_format
from .tools.manager import ToolManager
from .tools.computer import ToolResult
from .prompts import COMPUTER_USE, SYSTEM_PROMPT, MAC_SPECIFIC_NOTES

from .clients.oaicompat import OAICompatClient

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


class UITARSLoop(BaseLoop):
    """UI-TARS-specific implementation of the agent loop.

    This class extends BaseLoop to provide support for the UI-TARS model
    with computer control capabilities.
    """

    ###########################################
    # INITIALIZATION AND CONFIGURATION
    ###########################################

    def __init__(
        self,
        computer: Computer,
        api_key: str,
        model: str,
        provider_base_url: Optional[str] = "http://localhost:8000/v1",
        only_n_most_recent_images: Optional[int] = 2,
        base_dir: Optional[str] = "trajectories",
        max_retries: int = 3,
        retry_delay: float = 1.0,
        save_trajectory: bool = True,
        **kwargs,
    ):
        """Initialize the loop.

        Args:
            computer: Computer instance
            api_key: API key (may not be needed for local endpoints)
            model: Model name (e.g., "ui-tars")
            provider_base_url: Base URL for the API provider
            only_n_most_recent_images: Maximum number of recent screenshots to include in API requests
            base_dir: Base directory for saving experiment data
            max_retries: Maximum number of retries for API calls
            retry_delay: Delay between retries in seconds
            save_trajectory: Whether to save trajectory data
        """
        # Set provider before initializing base class
        self.provider = LLMProvider.OAICOMPAT
        self.provider_base_url = provider_base_url

        # Initialize message manager with image retention config
        self.message_manager = StandardMessageManager(
            config=ImageRetentionConfig(num_images_to_keep=only_n_most_recent_images)
        )

        # Initialize base class (which will set up experiment manager)
        super().__init__(
            computer=computer,
            model=model,
            api_key=api_key,
            max_retries=max_retries,
            retry_delay=retry_delay,
            base_dir=base_dir,
            save_trajectory=save_trajectory,
            only_n_most_recent_images=only_n_most_recent_images,
            **kwargs,
        )

        # Set API client attributes
        self.client = None
        self.retry_count = 0

        # Initialize visualization helper
        self.viz_helper = VisualizationHelper(agent=self)

        # Initialize tool manager
        self.tool_manager = ToolManager(computer=computer)

        logger.info("UITARSLoop initialized with StandardMessageManager")

    async def initialize(self) -> None:
        """Initialize the loop by setting up tools and clients."""
        # Initialize base class
        await super().initialize()

        # Initialize tool manager with error handling
        try:
            logger.info("Initializing tool manager...")
            await self.tool_manager.initialize()
            logger.info("Tool manager initialized successfully.")
        except Exception as e:
            logger.error(f"Error initializing tool manager: {str(e)}")
            logger.warning("Will attempt to initialize tools on first use.")

        # Initialize client for the OAICompat provider
        try:
            await self.initialize_client()
        except Exception as e:
            logger.error(f"Error initializing client: {str(e)}")
            raise RuntimeError(f"Failed to initialize client: {str(e)}")

    ###########################################
    # CLIENT INITIALIZATION - IMPLEMENTING ABSTRACT METHOD
    ###########################################

    async def initialize_client(self) -> None:
        """Initialize the appropriate client.

        Implements abstract method from BaseLoop to set up the specific
        provider client (OAICompat for UI-TARS).
        """
        try:
            logger.info(f"Initializing OAICompat client for UI-TARS with model {self.model}...")

            self.client = OAICompatClient(
                api_key=self.api_key or "EMPTY",  # Local endpoints typically don't require an API key
                model=self.model,
                provider_base_url=self.provider_base_url,
            )

            logger.info(f"Initialized OAICompat client with model {self.model}")
        except Exception as e:
            logger.error(f"Error initializing client: {str(e)}")
            self.client = None
            raise RuntimeError(f"Failed to initialize client: {str(e)}")

    ###########################################
    # MESSAGE FORMATTING
    ###########################################

    def to_uitars_format(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Convert messages to UI-TARS compatible format.
        
        Args:
            messages: List of messages in standard format
            
        Returns:
            List of messages formatted for UI-TARS
        """
        # Create a copy of the messages to avoid modifying the original
        uitars_messages = copy.deepcopy(messages)
        
        # Find the first user message to modify
        first_user_idx = None
        instruction = ""
        
        for idx, msg in enumerate(uitars_messages):
            if msg.get("role") == "user":
                first_user_idx = idx
                content = msg.get("content", "")
                if isinstance(content, str):
                    instruction = content
                    break
                elif isinstance(content, list):
                    for item in content:
                        if item.get("type") == "text":
                            instruction = item.get("text", "")
                            break
                    if instruction:
                        break
        
        # Only modify the first user message if found
        if first_user_idx is not None and instruction:
            # Create the computer use prompt
            user_prompt = COMPUTER_USE.format(
                instruction='\n'.join([instruction, MAC_SPECIFIC_NOTES]),
                language="English"
            )
            
            # Replace the content of the first user message
            if isinstance(uitars_messages[first_user_idx].get("content", ""), str):
                uitars_messages[first_user_idx]["content"] = [{"type": "text", "text": user_prompt}]
            elif isinstance(uitars_messages[first_user_idx].get("content", ""), list):
                # Find and replace only the text part, keeping images
                for i, item in enumerate(uitars_messages[first_user_idx]["content"]):
                    if item.get("type") == "text":
                        uitars_messages[first_user_idx]["content"][i]["text"] = user_prompt
                        break
        
        # Add box tokens to assistant responses
        for idx, msg in enumerate(uitars_messages):
            if msg.get("role") == "assistant":
                content = msg.get("content", "")
                if content and isinstance(content, list):
                    for i, part in enumerate(content):
                        if part.get('type') == 'text':
                            uitars_messages[idx]["content"][i]["text"] = add_box_token(part['text'])
        
        return uitars_messages

    ###########################################
    # API CALL HANDLING
    ###########################################

    async def _make_api_call(self, messages: List[Dict[str, Any]], system_prompt: str) -> Any:
        """Make API call to provider with retry logic."""
        # Create new turn directory for this API call
        self._create_turn_dir()

        request_data = None
        last_error = None

        for attempt in range(self.max_retries):
            try:
                # Ensure client is initialized
                if self.client is None:
                    logger.info(
                        f"Client not initialized in _make_api_call (attempt {attempt+1}), initializing now..."
                    )
                    await self.initialize_client()
                    if self.client is None:
                        raise RuntimeError("Failed to initialize client")

                # Get messages in standard format from the message manager
                self.message_manager.messages = messages.copy()
                prepared_messages = self.message_manager.get_messages()
                
                # Convert messages to UI-TARS format
                uitars_messages = self.to_uitars_format(prepared_messages)
                
                # Log request
                request_data = {
                    "messages": uitars_messages,
                    "max_tokens": self.max_tokens,
                    "system": system_prompt,
                }

                self._log_api_call("request", request_data)

                # Make API call
                response = await self.client.run_interleaved(
                    messages=uitars_messages,
                    system=system_prompt,
                    max_tokens=self.max_tokens,
                )

                # Log success response
                self._log_api_call("response", request_data, response)

                return response

            except (ConnectError, ReadTimeout) as e:
                last_error = e
                logger.warning(
                    f"Connection error on attempt {attempt + 1}/{self.max_retries}: {str(e)}"
                )
                if attempt < self.max_retries - 1:
                    await asyncio.sleep(self.retry_delay * (attempt + 1))  # Exponential backoff
                    # Reset client on connection errors to force re-initialization
                    self.client = None
                continue

            except RuntimeError as e:
                # Handle client initialization errors specifically
                last_error = e
                self._log_api_call("error", request_data, error=e)
                logger.error(
                    f"Client initialization error (attempt {attempt + 1}/{self.max_retries}): {str(e)}"
                )
                if attempt < self.max_retries - 1:
                    # Reset client to force re-initialization
                    self.client = None
                    await asyncio.sleep(self.retry_delay)
                continue

            except Exception as e:
                # Log unexpected error
                last_error = e
                self._log_api_call("error", request_data, error=e)
                logger.error(f"Unexpected error in API call: {str(e)}")
                if attempt < self.max_retries - 1:
                    await asyncio.sleep(self.retry_delay)
                continue

        # If we get here, all retries failed
        error_message = f"API call failed after {self.max_retries} attempts"
        if last_error:
            error_message += f": {str(last_error)}"

        logger.error(error_message)
        raise RuntimeError(error_message)

    ###########################################
    # RESPONSE AND ACTION HANDLING
    ###########################################

    async def _handle_response(
        self, response: Any, messages: List[Dict[str, Any]]
    ) -> Tuple[bool, bool]:
        """Handle API response.

        Args:
            response: API response
            messages: List of messages to update

        Returns:
            Tuple of (should_continue, action_screenshot_saved)
        """
        action_screenshot_saved = False

        try:
            # Step 1: Extract the raw response text
            raw_text = None
            
            try:
                # OpenAI-compatible response format
                raw_text = response["choices"][0]["message"]["content"]
            except (KeyError, TypeError, IndexError) as e:
                logger.error(f"Invalid response format: {str(e)}")
                return True, action_screenshot_saved

            # Step 2: Add the response to message history
            self.message_manager.add_assistant_message([{"type": "text", "text": raw_text}])
            
            # Step 3: Parse actions from the response
            parsed_actions = parse_actions(raw_text)
            
            if not parsed_actions:
                logger.warning("No action found in the response")
                return True, action_screenshot_saved
            
            # Step 4: Execute each action
            for action in parsed_actions:
                action_type = None
                
                # Handle "finished" action
                if action.startswith("finished"):
                    logger.info("Agent completed the task")
                    return False, action_screenshot_saved
                
                # Process other action types (click, type, etc.)
                try:
                    # Parse action parameters using the utility function
                    action_name, tool_args = parse_action_parameters(action)
                    
                    if not action_name:
                        logger.warning(f"Could not parse action: {action}")
                        continue

                    # Mark actions that would create screenshots
                    if action_name in ["click", "left_double", "right_single", "drag", "scroll"]:
                        action_screenshot_saved = True
                    
                    # Execute the tool with prepared arguments
                    await self._ensure_tools_initialized()
                    
                    # Let's log what we're about to execute for debugging
                    logger.info(f"Executing computer tool with arguments: {tool_args}")
                    
                    result = await self.tool_manager.execute_tool(name="computer", tool_input=tool_args)
                    
                    # Handle the result
                    if hasattr(result, "error") and result.error:
                        logger.error(f"Error executing tool: {result.error}")
                    else:
                        # Action was successful
                        logger.info(f"Successfully executed {action_name}")
                        
                        # Save screenshot if one was returned and we haven't already saved one
                        if hasattr(result, "base64_image") and result.base64_image:
                            self._save_screenshot(result.base64_image, action_type=action_name)
                            action_screenshot_saved = True
                    
                except Exception as e:
                    logger.error(f"Error executing action {action}: {str(e)}")
            
            # Continue the loop if there are actions to process
            return True, action_screenshot_saved

        except Exception as e:
            logger.error(f"Error handling response: {str(e)}")
            # Add error message using the message manager
            error_message = [{"type": "text", "text": f"Error: {str(e)}"}]
            self.message_manager.add_assistant_message(error_message)
            raise

    ###########################################
    # SCREEN HANDLING
    ###########################################

    async def _get_current_screen(self, save_screenshot: bool = True) -> str:
        """Get the current screen as a base64 encoded image.

        Args:
            save_screenshot: Whether to save the screenshot

        Returns:
            Base64 encoded screenshot
        """
        try:
            # Take a screenshot
            screenshot = await self.computer.interface.screenshot()
            
            # Convert to base64
            img_base64 = base64.b64encode(screenshot).decode("utf-8")
            
            # Process screenshot through hooks and save if needed
            await self.handle_screenshot(img_base64, action_type="state")
            
            # Save screenshot if requested
            if save_screenshot and self.save_trajectory:
                self._save_screenshot(img_base64, action_type="state")
                
            return img_base64
            
        except Exception as e:
            logger.error(f"Error getting current screen: {str(e)}")
            raise

    ###########################################
    # SYSTEM PROMPT
    ###########################################

    def _get_system_prompt(self) -> str:
        """Get the system prompt for the model."""
        return SYSTEM_PROMPT

    ###########################################
    # MAIN LOOP - IMPLEMENTING ABSTRACT METHOD
    ###########################################

    async def run(self, messages: List[Dict[str, Any]]) -> AsyncGenerator[Dict[str, Any], None]:
        """Run the agent loop with provided messages.

        Args:
            messages: List of messages in standard OpenAI format

        Yields:
            Agent response format
        """
        # Initialize the message manager with the provided messages
        self.message_manager.messages = messages.copy()
        logger.info(f"Starting UITARSLoop run with {len(self.message_manager.messages)} messages")

        # Continue running until explicitly told to stop
        running = True
        turn_created = False
        # Track if an action-specific screenshot has been saved this turn
        action_screenshot_saved = False

        attempt = 0
        max_attempts = 3

        while running and attempt < max_attempts:
            try:
                # Create a new turn directory if it's not already created
                if not turn_created:
                    self._create_turn_dir()
                    turn_created = True

                # Ensure client is initialized
                if self.client is None:
                    logger.info("Initializing client...")
                    await self.initialize_client()
                    if self.client is None:
                        raise RuntimeError("Failed to initialize client")
                    logger.info("Client initialized successfully")

                # Get current screen
                base64_screenshot = await self._get_current_screen()
                
                # Add screenshot to message history
                self.message_manager.add_user_message(
                    [
                        {
                            "type": "image_url",
                            "image_url": {"url": f"data:image/png;base64,{base64_screenshot}"},
                        }
                    ]
                )
                logger.info("Added screenshot to message history")

                # Get system prompt
                system_prompt = self._get_system_prompt()

                # Make API call with retries
                response = await self._make_api_call(
                    self.message_manager.messages, system_prompt
                )

                # Handle the response (may execute actions)
                # Returns: (should_continue, action_screenshot_saved)
                should_continue, new_screenshot_saved = await self._handle_response(
                    response, self.message_manager.messages
                )

                # Update whether an action screenshot was saved this turn
                action_screenshot_saved = action_screenshot_saved or new_screenshot_saved
                
                agent_response = await to_agent_response_format(
                    response,
                    messages,
                    model=self.model,
                )
                yield agent_response
                
                # Check if we should continue this conversation
                running = should_continue

                # Create a new turn directory if we're continuing
                if running:
                    turn_created = False

                # Reset attempt counter on success
                attempt = 0

            except Exception as e:
                attempt += 1
                error_msg = f"Error in run method (attempt {attempt}/{max_attempts}): {str(e)}"
                logger.error(error_msg)

                # If this is our last attempt, provide more info about the error
                if attempt >= max_attempts:
                    logger.error(f"Maximum retry attempts reached. Last error was: {str(e)}")

                yield {
                    "role": "assistant",
                    "content": f"Error: {str(e)}",
                    "metadata": {"title": "❌ Error"},
                }

                # Create a brief delay before retrying
                await asyncio.sleep(1)

    ###########################################
    # UTILITY METHODS
    ###########################################

    async def _ensure_tools_initialized(self) -> None:
        """Ensure the tool manager and tools are initialized before use."""
        if not hasattr(self.tool_manager, "tools") or self.tool_manager.tools is None:
            logger.info("Tools not initialized. Initializing now...")
            await self.tool_manager.initialize()
            logger.info("Tools initialized successfully.")

    async def process_model_response(self, response_text: str) -> Optional[Dict[str, Any]]:
        """Process model response to extract tool calls.

        Args:
            response_text: Model response text

        Returns:
            Extracted tool information, or None if no tool call was found
        """
        # UI-TARS doesn't use the standard tool call format, so we parse its actions differently
        parsed_actions = parse_actions(response_text)
        
        if parsed_actions:
            return {"actions": parsed_actions}
        
        return None
