"""Main entry point for computer agents."""

import asyncio
import logging
import os
from typing import AsyncGenerator, Optional

from computer import Computer

from .factory import LoopFactory
from .provider_config import DEFAULT_MODELS, ENV_VARS
from .types import LLM, AgentLoop, AgentResponse

logger = logging.getLogger(__name__)

class ComputerAgent:
    """A computer agent that can perform automated tasks using natural language instructions."""

    def __init__(
        self,
        computer: Computer,
        model: LLM,
        loop: AgentLoop,
        max_retries: int = 3,
        screenshot_dir: Optional[str] = None,
        log_dir: Optional[str] = None,
        api_key: Optional[str] = None,
        save_trajectory: bool = True,
        trajectory_dir: str = "trajectories",
        only_n_most_recent_images: Optional[int] = None,
        verbosity: int = logging.INFO,
    ):
        """Initialize the ComputerAgent.

        Args:
            computer: Computer instance. If not provided, one will be created with default settings.
            max_retries: Maximum number of retry attempts.
            screenshot_dir: Directory to save screenshots.
            log_dir: Directory to save logs (set to None to disable logging to files).
            model: LLM object containing provider and model name. Takes precedence over provider/model_name.
            provider: The AI provider to use (e.g., LLMProvider.ANTHROPIC). Only used if model is None.
            api_key: The API key for the provider. If not provided, will look for environment variable.
            model_name: The model name to use. Only used if model is None.
            save_trajectory: Whether to save the trajectory.
            trajectory_dir: Directory to save the trajectory.
            only_n_most_recent_images: Maximum number of recent screenshots to include in API requests.
            verbosity: Logging level.
        """
        # Basic agent configuration
        self.max_retries = max_retries
        self.computer = computer
        self.queue = asyncio.Queue()
        self.screenshot_dir = screenshot_dir
        self.log_dir = log_dir
        self._retry_count = 0
        self._initialized = False
        self._in_context = False

        # Set logging level
        logger.setLevel(verbosity)

        # Setup logging
        if self.log_dir:
            os.makedirs(self.log_dir, exist_ok=True)
            logger.info(f"Created logs directory: {self.log_dir}")

        # Setup screenshots directory
        if self.screenshot_dir:
            os.makedirs(self.screenshot_dir, exist_ok=True)
            logger.info(f"Created screenshots directory: {self.screenshot_dir}")

        # Use the provided LLM object
        self.provider = model.provider
        actual_model_name = model.name or DEFAULT_MODELS.get(self.provider, "")
        self.provider_base_url = getattr(model, "provider_base_url", None)

        # Ensure we have a valid model name
        if not actual_model_name:
            actual_model_name = DEFAULT_MODELS.get(self.provider, "")
            if not actual_model_name:
                raise ValueError(
                    f"No model specified for provider {self.provider} and no default found"
                )

        # Get API key from environment if not provided
        actual_api_key = api_key or os.environ.get(ENV_VARS[self.provider], "")
        # Ollama and OpenAI-compatible APIs typically don't require an API key
        if (
            not actual_api_key
            and str(self.provider) not in ["ollama", "oaicompat"]
            and ENV_VARS[self.provider] != "none"
        ):
            raise ValueError(f"No API key provided for {self.provider}")

        # Create the appropriate loop using the factory
        try:
            # Let the factory create the appropriate loop with needed components
            self._loop = LoopFactory.create_loop(
                loop_type=loop,
                provider=self.provider,
                computer=self.computer,
                model_name=actual_model_name,
                api_key=actual_api_key,
                save_trajectory=save_trajectory,
                trajectory_dir=trajectory_dir,
                only_n_most_recent_images=only_n_most_recent_images,
                provider_base_url=self.provider_base_url,
            )
        except ValueError as e:
            logger.error(f"Failed to create loop: {str(e)}")
            raise

        # Initialize the message manager from the loop
        self.message_manager = self._loop.message_manager

        logger.info(
            f"ComputerAgent initialized with provider: {self.provider}, model: {actual_model_name}"
        )

    async def __aenter__(self):
        """Initialize the agent when used as a context manager."""
        logger.info("Entering ComputerAgent context")
        self._in_context = True

        # In case the computer wasn't initialized
        try:
            # Initialize the computer only if not already initialized
            logger.info("Checking if computer is already initialized...")
            if not self.computer._initialized:
                logger.info("Initializing computer in __aenter__...")
                # Use the computer's __aenter__ directly instead of calling run()
                await self.computer.__aenter__()
                logger.info("Computer initialized in __aenter__")
            else:
                logger.info("Computer already initialized, skipping initialization")

        except Exception as e:
            logger.error(f"Error initializing computer in __aenter__: {str(e)}")
            raise

        await self.initialize()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Cleanup agent resources if needed."""
        logger.info("Cleaning up agent resources")
        self._in_context = False

        # Do any necessary cleanup
        # We're not shutting down the computer here as it might be shared
        # Just log that we're exiting
        if exc_type:
            logger.error(f"Exiting agent context with error: {exc_type.__name__}: {exc_val}")
        else:
            logger.info("Exiting agent context normally")

        # If we have a queue, make sure to signal it's done
        if hasattr(self, "queue") and self.queue:
            await self.queue.put(None)  # Signal that we're done

    async def initialize(self) -> None:
        """Initialize the agent and its components."""
        if not self._initialized:
            # Always initialize the computer if available
            if self.computer and not self.computer._initialized:
                await self.computer.run()
            self._initialized = True

    async def run(self, task: str) -> AsyncGenerator[AgentResponse, None]:
        """Run a task using the computer agent.

        Args:
            task: Task description

        Yields:
            Agent response format
        """
        try:
            logger.info(f"Running task: {task}")
            logger.info(
                f"Message history before task has {len(self.message_manager.messages)} messages"
            )

            # Initialize the computer if needed
            if not self._initialized:
                await self.initialize()

            # Add task as a user message using the message manager
            self.message_manager.add_user_message([{"type": "text", "text": task}])
            logger.info(
                f"Added task message. Message history now has {len(self.message_manager.messages)} messages"
            )

            # Pass properly formatted messages to the loop
            if self._loop is None:
                logger.error("Loop not initialized properly")
                yield {"error": "Loop not initialized properly"}
                return

            # Execute the task and yield results
            async for result in self._loop.run(self.message_manager.messages):
                yield result

        except Exception as e:
            logger.error(f"Error in agent run method: {str(e)}")
            yield {
                "role": "assistant",
                "content": f"Error: {str(e)}",
                "metadata": {"title": "❌ Error"},
            }
