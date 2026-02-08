"""Omni-specific agent loop implementation."""

import asyncio
import json
import logging
import os
import re
from typing import Any, AsyncGenerator, Dict, List, Optional, Tuple, cast

from computer import Computer
from httpx import ConnectError, ReadTimeout

from ...core.base import BaseLoop
from ...core.messages import ImageRetentionConfig, StandardMessageManager
from ...core.types import AgentResponse, LLMProvider
from ...core.visualization import VisualizationHelper
from .api_handler import OmniAPIHandler
from .clients.anthropic import AnthropicClient
from .clients.oaicompat import OAICompatClient
from .clients.ollama import OllamaClient
from .clients.openai import OpenAIClient
from .parser import OmniParser, ParseResult
from .prompts import SYSTEM_PROMPT
from .tools import ToolResult
from .tools.manager import ToolManager
from .utils import to_openai_agent_response_format

logger = logging.getLogger(__name__)

def extract_data(input_string: str, data_type: str) -> str:
    """Extract content from code blocks."""
    pattern = f"```{data_type}" + r"(.*?)(```|$)"
    matches = re.findall(pattern, input_string, re.DOTALL)
    return matches[0][0].strip() if matches else input_string


class OmniLoop(BaseLoop):
    """Omni-specific implementation of the agent loop.

    This class extends BaseLoop to provide support for multimodal models
    from various providers (OpenAI, Anthropic, etc.) with UI parsing
    and desktop automation capabilities.
    """

    ###########################################
    # INITIALIZATION AND CONFIGURATION
    ###########################################

    def __init__(
        self,
        parser: OmniParser,
        provider: LLMProvider,
        api_key: str,
        model: str,
        computer: Computer,
        only_n_most_recent_images: Optional[int] = 2,
        base_dir: Optional[str] = "trajectories",
        max_retries: int = 3,
        retry_delay: float = 1.0,
        save_trajectory: bool = True,
        provider_base_url: Optional[str] = None,
        **kwargs,
    ):
        """Initialize the loop.

        Args:
            parser: Parser instance
            provider: API provider
            api_key: API key
            model: Model name
            computer: Computer instance
            only_n_most_recent_images: Maximum number of recent screenshots to include in API requests
            base_dir: Base directory for saving experiment data
            max_retries: Maximum number of retries for API calls
            retry_delay: Delay between retries in seconds
            save_trajectory: Whether to save trajectory data
            provider_base_url: Base URL for the API provider (used for OAICOMPAT)
        """
        # Set parser and provider before initializing base class
        self.parser = parser
        self.provider = provider
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
        self.loop_task = None  # Store the loop task for cancellation

        # Initialize handlers
        self.api_handler = OmniAPIHandler(loop=self)
        self.viz_helper = VisualizationHelper(agent=self)

        # Initialize tool manager
        self.tool_manager = ToolManager(computer=computer, provider=provider)

        logger.info("OmniLoop initialized with StandardMessageManager")

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

        # Initialize API clients based on provider
        if self.provider == LLMProvider.ANTHROPIC:
            self.client = AnthropicClient(
                api_key=self.api_key,
                model=self.model,
            )
        elif self.provider == LLMProvider.OPENAI:
            self.client = OpenAIClient(
                api_key=self.api_key,
                model=self.model,
            )
        elif self.provider == LLMProvider.OLLAMA:
            self.client = OllamaClient(
                api_key=self.api_key,
                model=self.model,
            )
        elif self.provider == LLMProvider.OAICOMPAT:
            self.client = OAICompatClient(
                api_key=self.api_key or "EMPTY",  # Local endpoints typically don't require an API key
                model=self.model,
                provider_base_url=self.provider_base_url,
            )
        else:
            raise ValueError(f"Unsupported provider: {self.provider}")

    ###########################################
    # CLIENT INITIALIZATION - IMPLEMENTING ABSTRACT METHOD
    ###########################################

    async def initialize_client(self) -> None:
        """Initialize the appropriate client based on provider.

        Implements abstract method from BaseLoop to set up the specific
        provider client (OpenAI, Anthropic, etc.).
        """
        try:
            logger.info(f"Initializing {self.provider} client with model {self.model}...")

            if self.provider == LLMProvider.OPENAI:
                self.client = OpenAIClient(api_key=self.api_key, model=self.model)
            elif self.provider == LLMProvider.ANTHROPIC:
                self.client = AnthropicClient(
                    api_key=self.api_key,
                    model=self.model,
                    max_retries=self.max_retries,
                    retry_delay=self.retry_delay,
                )
            elif self.provider == LLMProvider.OLLAMA:
                self.client = OllamaClient(
                    api_key=self.api_key,
                    model=self.model,
                )
            elif self.provider == LLMProvider.OAICOMPAT:
                self.client = OAICompatClient(
                    api_key=self.api_key or "EMPTY",  # Local endpoints typically don't require an API key
                    model=self.model,
                    provider_base_url=self.provider_base_url,
                )
            else:
                raise ValueError(f"Unsupported provider: {self.provider}")

            logger.info(f"Initialized {self.provider} client with model {self.model}")
        except Exception as e:
            logger.error(f"Error initializing client: {str(e)}")
            self.client = None
            raise RuntimeError(f"Failed to initialize client: {str(e)}")

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

                # Special handling for Anthropic
                if self.provider == LLMProvider.ANTHROPIC:
                    # Convert to Anthropic format
                    anthropic_messages, anthropic_system = self.message_manager.to_anthropic_format(
                        prepared_messages
                    )

                    # Filter out any empty/invalid messages
                    filtered_messages = [
                        msg
                        for msg in anthropic_messages
                        if msg.get("role") in ["user", "assistant"]
                    ]

                    # Ensure there's at least one message for Anthropic
                    if not filtered_messages:
                        logger.warning(
                            "No valid messages found for Anthropic API call. Adding a default user message."
                        )
                        filtered_messages = [
                            {
                                "role": "user",
                                "content": [
                                    {"type": "text", "text": "Please help with this task."}
                                ],
                            }
                        ]

                    # Combine system prompts if needed
                    final_system_prompt = anthropic_system or system_prompt

                    # Log request
                    request_data = {
                        "messages": filtered_messages,
                        "max_tokens": self.max_tokens,
                        "system": final_system_prompt,
                    }

                    self._log_api_call("request", request_data)

                    # Make API call
                    response = await self.client.run_interleaved(
                        messages=filtered_messages,
                        system=final_system_prompt,
                        max_tokens=self.max_tokens,
                    )
                else:
                    # For OpenAI and others, use standard format directly
                    # Log request
                    request_data = {
                        "messages": prepared_messages,
                        "max_tokens": self.max_tokens,
                        "system": system_prompt,
                    }

                    self._log_api_call("request", request_data)

                    # Make API call
                    response = await self.client.run_interleaved(
                        messages=prepared_messages,
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
        self, response: Any, messages: List[Dict[str, Any]], parsed_screen: ParseResult
    ) -> Tuple[bool, bool]:
        """Handle API response.

        Args:
            response: API response
            messages: List of messages to update
            parsed_screen: Current parsed screen information

        Returns:
            Tuple of (should_continue, action_screenshot_saved)
        """
        action_screenshot_saved = False

        # Helper function to safely add assistant messages using the message manager
        def add_assistant_message(content):
            if isinstance(content, str):
                # Convert string to proper format
                formatted_content = [{"type": "text", "text": content}]
                self.message_manager.add_assistant_message(formatted_content)
                logger.info("Added formatted text assistant message")
            elif isinstance(content, list):
                # Already in proper format
                self.message_manager.add_assistant_message(content)
                logger.info("Added structured assistant message")
            else:
                # Default case - convert to string
                formatted_content = [{"type": "text", "text": str(content)}]
                self.message_manager.add_assistant_message(formatted_content)
                logger.info("Added converted assistant message")

        try:
            # Step 1: Normalize response to standard format based on provider
            standard_content = []
            raw_text = None

            # Convert response to standardized content based on provider
            if self.provider == LLMProvider.ANTHROPIC:
                if hasattr(response, "content") and isinstance(response.content, list):
                    # Convert Anthropic response to standard format
                    for block in response.content:
                        if hasattr(block, "type"):
                            if block.type == "text":
                                standard_content.append({"type": "text", "text": block.text})
                                # Store raw text for JSON parsing
                                if raw_text is None:
                                    raw_text = block.text
                                else:
                                    raw_text += "\n" + block.text
                            else:
                                # Add other block types
                                block_dict = {}
                                for key, value in vars(block).items():
                                    if not key.startswith("_"):
                                        block_dict[key] = value
                                standard_content.append(block_dict)
                else:
                    logger.warning("Invalid Anthropic response format")
                    return True, action_screenshot_saved
            elif self.provider == LLMProvider.OLLAMA:
                try:
                    raw_text = response["message"]["content"]
                    standard_content = [{"type": "text", "text": raw_text}]
                except (KeyError, TypeError, IndexError) as e:
                    logger.error(f"Invalid response format: {str(e)}")
                    return True, action_screenshot_saved
            elif self.provider == LLMProvider.OAICOMPAT:
                try:
                    # OpenAI-compatible response format
                    raw_text = response["choices"][0]["message"]["content"]
                    standard_content = [{"type": "text", "text": raw_text}]
                except (KeyError, TypeError, IndexError) as e:
                    logger.error(f"Invalid response format: {str(e)}")
                    return True, action_screenshot_saved
            else:
                # Assume OpenAI or compatible format
                try:
                    raw_text = response["choices"][0]["message"]["content"]
                    standard_content = [{"type": "text", "text": raw_text}]
                except (KeyError, TypeError, IndexError) as e:
                    logger.error(f"Invalid response format: {str(e)}")
                    return True, action_screenshot_saved

            # Step 2: Add the normalized response to message history
            add_assistant_message(standard_content)

            # Step 3: Extract JSON from the content for action execution
            parsed_content = None

            # If we have raw text, try to extract JSON from it
            if raw_text:
                # Try different approaches to extract JSON
                try:
                    # First try to parse the whole content as JSON
                    parsed_content = json.loads(raw_text)
                    logger.info("Successfully parsed whole content as JSON")
                except json.JSONDecodeError:
                    try:
                        # Try to find JSON block
                        json_content = extract_data(raw_text, "json")
                        parsed_content = json.loads(json_content)
                        logger.info("Successfully parsed JSON from code block")
                    except (json.JSONDecodeError, IndexError):
                        try:
                            # Look for JSON object pattern
                            import re  # Local import to ensure availability

                            json_pattern = r"\{[^}]+\}"
                            json_match = re.search(json_pattern, raw_text)
                            if json_match:
                                json_str = json_match.group(0)
                                parsed_content = json.loads(json_str)
                                logger.info("Successfully parsed JSON from text")
                            else:
                                logger.error(f"No JSON found in content")
                                return True, action_screenshot_saved
                        except json.JSONDecodeError as e:
                            # Try to sanitize the JSON string and retry
                            try:
                                # Remove or replace invalid control characters
                                import re  # Local import to ensure availability

                                sanitized_text = re.sub(r"[\x00-\x1F\x7F]", "", raw_text)
                                # Try parsing again with sanitized text
                                parsed_content = json.loads(sanitized_text)
                                logger.info(
                                    "Successfully parsed JSON after sanitizing control characters"
                                )
                            except json.JSONDecodeError:
                                logger.error(f"Failed to parse JSON from text: {str(e)}")
                                return True, action_screenshot_saved

            # Step 4: Process the parsed content if available
            if parsed_content:
                # Clean up Box ID format
                if "Box ID" in parsed_content and isinstance(parsed_content["Box ID"], str):
                    parsed_content["Box ID"] = parsed_content["Box ID"].replace("Box #", "")

                # Add any explanatory text as reasoning if not present
                if "Explanation" not in parsed_content and raw_text:
                    # Extract any text before the JSON as reasoning
                    text_before_json = raw_text.split("{")[0].strip()
                    if text_before_json:
                        parsed_content["Explanation"] = text_before_json

                # Log the parsed content for debugging
                logger.info(f"Parsed content: {json.dumps(parsed_content, indent=2)}")

                # Step 5: Execute the action
                try:
                    # Execute action using the common helper method
                    should_continue, action_screenshot_saved = (
                        await self._execute_action_with_tools(
                            parsed_content, cast(ParseResult, parsed_screen)
                        )
                    )

                    # Check if task is complete
                    if parsed_content.get("Action") == "None":
                        return False, action_screenshot_saved
                    return should_continue, action_screenshot_saved
                except Exception as e:
                    logger.error(f"Error executing action: {str(e)}")
                    # Update the last assistant message with error
                    error_message = [{"type": "text", "text": f"Error executing action: {str(e)}"}]
                    # Replace the last assistant message with the error
                    self.message_manager.add_assistant_message(error_message)
                    return False, action_screenshot_saved

            return True, action_screenshot_saved

        except Exception as e:
            logger.error(f"Error handling response: {str(e)}")
            # Add error message using the message manager
            error_message = [{"type": "text", "text": f"Error: {str(e)}"}]
            self.message_manager.add_assistant_message(error_message)
            raise

    ###########################################
    # SCREEN PARSING - IMPLEMENTING ABSTRACT METHOD
    ###########################################

    async def _get_parsed_screen_som(self, save_screenshot: bool = True) -> ParseResult:
        """Get parsed screen information with Screen Object Model.

        Extends the base class method to use the OmniParser to parse the screen
        and extract UI elements.

        Args:
            save_screenshot: Whether to save the screenshot (set to False when screenshots will be saved elsewhere)

        Returns:
            ParseResult containing screen information and elements
        """
        try:
            # Use the parser's parse_screen method which handles the screenshot internally
            parsed_screen = await self.parser.parse_screen(computer=self.computer)

            # Log information about the parsed results
            logger.info(
                f"Parsed screen with {len(parsed_screen.elements) if parsed_screen.elements else 0} elements"
            )

            # Save screenshot if requested and if we have image data
            if save_screenshot and self.save_trajectory and parsed_screen.annotated_image_base64:
                try:
                    # Extract just the image data (remove data:image/png;base64, prefix)
                    img_data = parsed_screen.annotated_image_base64
                    if "," in img_data:
                        img_data = img_data.split(",")[1]
                    
                    # Process screenshot through hooks and save if needed
                    await self.handle_screenshot(img_data, action_type="state", parsed_screen=parsed_screen)
                    
                    # Save with a generic "state" action type to indicate this is the current screen state
                    self._save_screenshot(img_data, action_type="state")
                except Exception as e:
                    logger.error(f"Error saving screenshot: {str(e)}")

            return parsed_screen

        except Exception as e:
            logger.error(f"Error getting parsed screen: {str(e)}")
            raise

    def _get_system_prompt(self) -> str:
        """Get the system prompt for the model."""
        return SYSTEM_PROMPT

    ###########################################
    # MAIN LOOP - IMPLEMENTING ABSTRACT METHOD
    ###########################################

    async def run(self, messages: List[Dict[str, Any]]) -> AsyncGenerator[AgentResponse, None]:
        """Run the agent loop with provided messages.

        Args:
            messages: List of messages in standard OpenAI format

        Yields:
            Agent response format
        """
        try:
            logger.info(f"Starting OmniLoop run with {len(messages)} messages")
            
            # Initialize the message manager with the provided messages
            self.message_manager.messages = messages.copy()
            
            # Create queue for response streaming
            queue = asyncio.Queue()
            
            # Start loop in background task
            self.loop_task = asyncio.create_task(self._run_loop(queue, messages))

            # Process and yield messages as they arrive
            while True:
                try:
                    item = await queue.get()
                    if item is None:  # Stop signal
                        break
                    yield item
                    queue.task_done()
                except Exception as e:
                    logger.error(f"Error processing queue item: {str(e)}")
                    continue

            # Wait for loop to complete
            await self.loop_task

            # Send completion message
            yield {
                "role": "assistant",
                "content": "Task completed successfully.",
                "metadata": {"title": "✅ Complete"},
            }

        except Exception as e:
            logger.error(f"Error in run method: {str(e)}")
            yield {
                "role": "assistant",
                "content": f"Error: {str(e)}",
                "metadata": {"title": "❌ Error"},
            }
            
    async def _run_loop(self, queue: asyncio.Queue, messages: List[Dict[str, Any]]) -> None:
        """Internal method to run the agent loop with provided messages.
        
        Args:
            queue: Queue to put responses into
            messages: List of messages in standard OpenAI format
        """
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

                # Get up-to-date screen information
                parsed_screen = await self._get_parsed_screen_som()

                # Process screen info and update messages in standard format
                try:
                    # Get image from parsed screen
                    image = parsed_screen.annotated_image_base64 or None
                    if image:
                        # Save elements as JSON if we have a turn directory
                        if self.current_turn_dir and hasattr(parsed_screen, "elements"):
                            elements_path = os.path.join(self.current_turn_dir, "elements.json")
                            with open(elements_path, "w") as f:
                                # Convert elements to dicts for JSON serialization
                                elements_json = [
                                    elem.model_dump() for elem in parsed_screen.elements
                                ]
                                json.dump(elements_json, f, indent=2)
                                logger.info(f"Saved elements to {elements_path}")

                        # Remove data URL prefix if present
                        if "," in image:
                            image = image.split(",")[1]

                        # Add screenshot to message history using message manager
                        self.message_manager.add_user_message(
                            [
                                {
                                    "type": "image_url",
                                    "image_url": {"url": f"data:image/png;base64,{image}"},
                                }
                            ]
                        )
                        logger.info("Added screenshot to message history")
                except Exception as e:
                    logger.error(f"Error processing screen info: {str(e)}")
                    raise

                # Get system prompt
                system_prompt = self._get_system_prompt()

                # Make API call with retries using the APIHandler
                response = await self.api_handler.make_api_call(
                    self.message_manager.messages, system_prompt
                )

                # Handle the response (may execute actions)
                # Returns: (should_continue, action_screenshot_saved)
                should_continue, new_screenshot_saved = await self._handle_response(
                    response, self.message_manager.messages, parsed_screen
                )

                # Update whether an action screenshot was saved this turn
                action_screenshot_saved = action_screenshot_saved or new_screenshot_saved

                # Create OpenAI-compatible response format using utility function
                openai_compatible_response = await to_openai_agent_response_format(
                    response=response,
                    messages=self.message_manager.messages,
                    model=self.model,
                    parsed_screen=parsed_screen,
                    parser=self.parser
                )
                # Log standardized response for ease of parsing
                self._log_api_call("agent_response", request=None, response=openai_compatible_response)

                # Put the response in the queue
                await queue.put(openai_compatible_response)

                # Check if we should continue this conversation
                running = should_continue

                # Create a new turn directory if we're continuing
                if running:
                    turn_created = False

                # Reset attempt counter on success
                attempt = 0

            except Exception as e:
                attempt += 1
                error_msg = f"Error in _run_loop method (attempt {attempt}/{max_attempts}): {str(e)}"
                logger.error(error_msg)

                # If this is our last attempt, provide more info about the error
                if attempt >= max_attempts:
                    logger.error(f"Maximum retry attempts reached. Last error was: {str(e)}")

                await queue.put({
                    "role": "assistant",
                    "content": f"Error: {str(e)}",
                    "metadata": {"title": "❌ Error"},
                })

                # Create a brief delay before retrying
                await asyncio.sleep(1)
            finally:
                # Signal that we're done
                await queue.put(None)
                
    async def cancel(self) -> None:
        """Cancel the currently running agent loop task.
        
        This method stops the ongoing processing in the agent loop
        by cancelling the loop_task if it exists and is running.
        """
        if self.loop_task and not self.loop_task.done():
            logger.info("Cancelling Omni loop task")
            self.loop_task.cancel()
            try:
                # Wait for the task to be cancelled with a timeout
                await asyncio.wait_for(self.loop_task, timeout=2.0)
            except asyncio.TimeoutError:
                logger.warning("Timeout while waiting for loop task to cancel")
            except asyncio.CancelledError:
                logger.info("Loop task cancelled successfully")
            except Exception as e:
                logger.error(f"Error while cancelling loop task: {str(e)}")
            finally:
                logger.info("Omni loop task cancelled")
        else:
            logger.info("No active Omni loop task to cancel")

    async def process_model_response(self, response_text: str) -> Optional[Dict[str, Any]]:
        """Process model response to extract tool calls.

        Args:
            response_text: Model response text

        Returns:
            Extracted tool information, or None if no tool call was found
        """
        try:
            # Ensure tools are initialized before use
            await self._ensure_tools_initialized()

            # Look for tool use in the response
            if "function_call" in response_text or "tool_use" in response_text:
                # The extract_tool_call method should be implemented in the OmniAPIHandler
                # For now, we'll just use a simple approach
                # This will be replaced with the proper implementation
                tool_info = None
                if "function_call" in response_text:
                    # Extract function call params
                    try:
                        # Simple extraction - in real code this would be more robust
                        import json
                        import re

                        match = re.search(r'"function_call"\s*:\s*{([^}]+)}', response_text)
                        if match:
                            function_text = "{" + match.group(1) + "}"
                            tool_info = json.loads(function_text)
                    except Exception as e:
                        logger.error(f"Error extracting function call: {str(e)}")

                if tool_info:
                    try:
                        # Execute the tool
                        result = await self.tool_manager.execute_tool(
                            name=tool_info.get("name"), tool_input=tool_info.get("arguments", {})
                        )
                        # Handle the result
                        return {"tool_result": result}
                    except Exception as e:
                        error_msg = (
                            f"Error executing tool '{tool_info.get('name', 'unknown')}': {str(e)}"
                        )
                        logger.error(error_msg)
                        return {"tool_result": ToolResult(error=error_msg)}
        except Exception as e:
            logger.error(f"Error processing tool call: {str(e)}")

        return None

    async def process_response_with_tools(
        self, response_text: str, parsed_screen: Optional[ParseResult] = None
    ) -> Tuple[bool, str]:
        """Process model response and execute tools.

        Args:
            response_text: Model response text
            parsed_screen: Current parsed screen information (optional)

        Returns:
            Tuple of (action_taken, observation)
        """
        logger.info("Processing response with tools")

        # Process the response to extract tool calls
        tool_result = await self.process_model_response(response_text)

        if tool_result and "tool_result" in tool_result:
            # A tool was executed
            result = tool_result["tool_result"]
            if result.error:
                return False, f"ERROR: {result.error}"
            else:
                return True, result.output or "Tool executed successfully"

        # No action or tool call found
        return False, "No action taken - no tool call detected in response"

    ###########################################
    # UTILITY METHODS
    ###########################################

    async def _ensure_tools_initialized(self) -> None:
        """Ensure the tool manager and tools are initialized before use."""
        if not hasattr(self.tool_manager, "tools") or self.tool_manager.tools is None:
            logger.info("Tools not initialized. Initializing now...")
            await self.tool_manager.initialize()
            logger.info("Tools initialized successfully.")

    async def _execute_action_with_tools(
        self, action_data: Dict[str, Any], parsed_screen: ParseResult
    ) -> Tuple[bool, bool]:
        """Execute an action using the tools-based approach.

        Args:
            action_data: Dictionary containing action details
            parsed_screen: Current parsed screen information

        Returns:
            Tuple of (should_continue, action_screenshot_saved)
        """
        action_screenshot_saved = False
        action_type = None  # Initialize for possible use in post-action screenshot

        try:
            # Extract the action
            parsed_action = action_data.get("Action", "").lower()

            # Only process if we have a valid action
            if not parsed_action or parsed_action == "none":
                return False, action_screenshot_saved

            # Convert the parsed content to a format suitable for the tools system
            tool_name = "computer"  # Default to computer tool
            tool_args = {"action": parsed_action}

            # Add specific arguments based on action type
            if parsed_action in ["left_click", "right_click", "double_click", "move_cursor"]:
                # Calculate coordinates from Box ID using parser
                try:
                    box_id = int(action_data["Box ID"])
                    x, y = await self.parser.calculate_click_coordinates(
                        box_id, cast(ParseResult, parsed_screen)
                    )
                    tool_args["x"] = x
                    tool_args["y"] = y

                    # Visualize action if screenshot is available
                    if parsed_screen and parsed_screen.annotated_image_base64:
                        img_data = parsed_screen.annotated_image_base64
                        # Remove data URL prefix if present
                        if img_data.startswith("data:image"):
                            img_data = img_data.split(",")[1]
                        # Save visualization for coordinate-based actions
                        self.viz_helper.visualize_action(x, y, img_data)
                        action_screenshot_saved = True

                except (ValueError, KeyError) as e:
                    logger.error(f"Error processing Box ID: {str(e)}")
                    return False, action_screenshot_saved

            elif parsed_action == "type_text":
                tool_args["text"] = action_data.get("Value", "")
                # For type_text, store the value in the action type for screenshot naming
                action_type = f"type_{tool_args['text'][:20]}"  # Truncate if too long

            elif parsed_action == "press_key":
                tool_args["key"] = action_data.get("Value", "")
                action_type = f"press_{tool_args['key']}"

            elif parsed_action == "hotkey":
                value = action_data.get("Value", "")
                if isinstance(value, list):
                    tool_args["keys"] = value
                    action_type = f"hotkey_{'_'.join(value)}"
                else:
                    # Split string format like "command+space" into a list
                    keys = [k.strip() for k in value.lower().split("+")]
                    tool_args["keys"] = keys
                    action_type = f"hotkey_{value.replace('+', '_')}"

            elif parsed_action in ["scroll_down", "scroll_up"]:
                clicks = int(action_data.get("amount", 1))
                tool_args["amount"] = clicks
                action_type = f"scroll_{parsed_action.split('_')[1]}_{clicks}"

                # Visualize scrolling if screenshot is available
                if parsed_screen and parsed_screen.annotated_image_base64:
                    img_data = parsed_screen.annotated_image_base64
                    # Remove data URL prefix if present
                    if img_data.startswith("data:image"):
                        img_data = img_data.split(",")[1]
                    direction = "down" if parsed_action == "scroll_down" else "up"
                    # For scrolling, we save the visualization
                    self.viz_helper.visualize_scroll(direction, clicks, img_data)
                    action_screenshot_saved = True

            # Ensure tools are initialized before use
            await self._ensure_tools_initialized()

            # Execute tool with prepared arguments
            result = await self.tool_manager.execute_tool(name=tool_name, tool_input=tool_args)

            # Take a new screenshot after the action if we haven't already saved one
            if not action_screenshot_saved:
                try:
                    # Get a new screenshot after the action
                    new_parsed_screen = await self._get_parsed_screen_som(save_screenshot=False)
                    if new_parsed_screen and new_parsed_screen.annotated_image_base64:
                        img_data = new_parsed_screen.annotated_image_base64
                        # Remove data URL prefix if present
                        if img_data.startswith("data:image"):
                            img_data = img_data.split(",")[1]
                        # Save with action type if defined, otherwise use the action name
                        if action_type:
                            self._save_screenshot(img_data, action_type=action_type)
                        else:
                            self._save_screenshot(img_data, action_type=parsed_action)
                        action_screenshot_saved = True
                except Exception as screenshot_error:
                    logger.error(f"Error taking post-action screenshot: {str(screenshot_error)}")

            # Continue the loop if the action is not "None"
            return True, action_screenshot_saved

        except Exception as e:
            logger.error(f"Error executing action: {str(e)}")
            # Update the last assistant message with error
            error_message = [{"type": "text", "text": f"Error executing action: {str(e)}"}]
            # Replace the last assistant message with the error
            self.message_manager.add_assistant_message(error_message)
            return False, action_screenshot_saved
