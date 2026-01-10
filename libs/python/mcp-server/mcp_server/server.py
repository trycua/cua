import asyncio
import base64
import inspect
import logging
import os
import signal
import sys
import traceback
import uuid
from typing import Any, Dict, List, Optional, Tuple, Union

import anyio

# Configure logging to output to stderr for debug visibility
logging.basicConfig(
    level=logging.DEBUG,  # Changed to DEBUG
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    stream=sys.stderr,
)
logger = logging.getLogger("mcp-server")

# More visible startup message
logger.debug("MCP Server module loading...")

try:
    from mcp.server.fastmcp import Context, FastMCP

    # Use the canonical Image type
    from mcp.server.fastmcp.utilities.types import Image

    logger.debug("Successfully imported FastMCP")
except ImportError as e:
    logger.error(f"Failed to import FastMCP: {e}")
    traceback.print_exc(file=sys.stderr)
    sys.exit(1)

try:
    from agent import ComputerAgent
    from computer import Computer

    logger.debug("Successfully imported Computer and Agent modules")
except ImportError as e:
    logger.error(f"Failed to import Computer/Agent modules: {e}")
    traceback.print_exc(file=sys.stderr)
    sys.exit(1)

try:
    from .session_manager import (
        get_session_manager,
        initialize_session_manager,
        shutdown_session_manager,
    )

    logger.debug("Successfully imported session manager")
except ImportError as e:
    logger.error(f"Failed to import session manager: {e}")
    traceback.print_exc(file=sys.stderr)
    sys.exit(1)


def get_env_bool(key: str, default: bool = False) -> bool:
    """Get boolean value from environment variable."""
    return os.getenv(key, str(default)).lower() in ("true", "1", "yes")


async def _maybe_call_ctx_method(ctx: Context, method_name: str, *args, **kwargs) -> None:
    """Call a context helper if it exists, awaiting the result when necessary."""
    method = getattr(ctx, method_name, None)
    if not callable(method):
        return
    result = method(*args, **kwargs)
    if inspect.isawaitable(result):
        await result


def _normalise_message_content(content: Union[str, List[Dict[str, Any]]]) -> List[Dict[str, Any]]:
    """Normalise message content to a list of structured parts."""
    if isinstance(content, list):
        return content
    if content is None:
        return []
    return [{"type": "output_text", "text": str(content)}]


def _extract_text_from_content(content: Union[str, List[Dict[str, Any]]]) -> str:
    """Extract textual content for inclusion in the aggregated result string."""
    if isinstance(content, str):
        return content
    texts: List[str] = []
    for part in content or []:
        if not isinstance(part, dict):
            continue
        if part.get("type") in {"output_text", "text"} and part.get("text"):
            texts.append(str(part["text"]))
    return "\n".join(texts)


def _serialise_tool_content(content: Any) -> str:
    """Convert tool outputs into a string for aggregation."""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        texts: List[str] = []
        for part in content:
            if (
                isinstance(part, dict)
                and part.get("type") in {"output_text", "text"}
                and part.get("text")
            ):
                texts.append(str(part["text"]))
        if texts:
            return "\n".join(texts)
    if content is None:
        return ""
    return str(content)


def serve() -> FastMCP:
    """Create and configure the MCP server."""
    # NOTE: Do not pass model_config here; FastMCP 2.12.x doesn't support it.
    server = FastMCP(name="cua-agent")

    @server.tool(structured_output=False)
    async def screenshot_cua(ctx: Context, session_id: Optional[str] = None) -> Any:
        """
        Take a screenshot of the current MacOS VM screen and return the image.

        Args:
            session_id: Optional session ID for multi-client support. If not provided, a new session will be created.
        """
        session_manager = get_session_manager()

        async with session_manager.get_session(session_id) as session:
            screenshot = await session.computer.interface.screenshot()
            # Returning Image object is fine when structured_output=False
            return Image(format="png", data=screenshot)

    @server.tool(structured_output=False)
    async def run_cua_task(ctx: Context, task: str, session_id: Optional[str] = None) -> Any:
        """
        Run a Computer-Use Agent (Cua) task in a MacOS VM and return (combined text, final screenshot).

        Args:
            task: The task description for the agent to execute
            session_id: Optional session ID for multi-client support. If not provided, a new session will be created.
        """
        session_manager = get_session_manager()
        task_id = str(uuid.uuid4())

        try:
            logger.info(f"Starting Cua task: {task} (task_id: {task_id})")

            async with session_manager.get_session(session_id) as session:
                # Register this task with the session
                await session_manager.register_task(session.session_id, task_id)

                try:
                    # Get model name
                    model_name = os.getenv("CUA_MODEL_NAME", "anthropic/claude-sonnet-4-5-20250929")
                    logger.info(f"Using model: {model_name}")

                    # Create agent with the new v0.4.x API
                    agent = ComputerAgent(
                        model=model_name,
                        only_n_most_recent_images=int(os.getenv("CUA_MAX_IMAGES", "3")),
                        verbosity=logging.INFO,
                        tools=[session.computer],
                    )

                    messages = [{"role": "user", "content": task}]

                    # Collect all results
                    aggregated_messages: List[str] = []
                    async for result in agent.run(messages):
                        logger.info("Agent processing step")
                        ctx.info("Agent processing step")

                        outputs = result.get("output", [])
                        for output in outputs:
                            output_type = output.get("type")

                            if output_type == "message":
                                logger.debug("Streaming assistant message: %s", output)
                                content = _normalise_message_content(output.get("content"))
                                aggregated_text = _extract_text_from_content(content)
                                if aggregated_text:
                                    aggregated_messages.append(aggregated_text)
                                await _maybe_call_ctx_method(
                                    ctx,
                                    "yield_message",
                                    role=output.get("role", "assistant"),
                                    content=content,
                                )

                            elif output_type in {"tool_use", "computer_call", "function_call"}:
                                logger.debug("Streaming tool call: %s", output)
                                call_id = output.get("id") or output.get("call_id")
                                tool_name = output.get("name") or output.get("action", {}).get(
                                    "type"
                                )
                                tool_input = (
                                    output.get("input")
                                    or output.get("arguments")
                                    or output.get("action")
                                )
                                if call_id:
                                    await _maybe_call_ctx_method(
                                        ctx,
                                        "yield_tool_call",
                                        name=tool_name,
                                        call_id=call_id,
                                        input=tool_input,
                                    )

                            elif output_type in {
                                "tool_result",
                                "computer_call_output",
                                "function_call_output",
                            }:
                                logger.debug("Streaming tool output: %s", output)
                                call_id = output.get("call_id") or output.get("id")
                                content = output.get("content") or output.get("output")
                                aggregated_text = _serialise_tool_content(content)
                                if aggregated_text:
                                    aggregated_messages.append(aggregated_text)
                                if call_id:
                                    await _maybe_call_ctx_method(
                                        ctx,
                                        "yield_tool_output",
                                        call_id=call_id,
                                        output=content,
                                        is_error=output.get("status") == "failed"
                                        or output.get("is_error", False),
                                    )

                    logger.info("Cua task completed successfully")
                    ctx.info("Cua task completed successfully")

                    screenshot_image = Image(
                        format="png",
                        data=await session.computer.interface.screenshot(),
                    )

                    return (
                        "\n".join(aggregated_messages).strip()
                        or "Task completed with no text output.",
                        screenshot_image,
                    )

                finally:
                    # Unregister the task from the session
                    await session_manager.unregister_task(session.session_id, task_id)

        except Exception as e:
            error_msg = f"Error running Cua task: {str(e)}\n{traceback.format_exc()}"
            logger.error(error_msg)
            ctx.error(error_msg)

            # Try to get a screenshot from the session if available
            try:
                if session_id:
                    async with session_manager.get_session(session_id) as session:
                        screenshot = await session.computer.interface.screenshot()
                        return (
                            f"Error during task execution: {str(e)}",
                            Image(format="png", data=screenshot),
                        )
            except Exception:
                pass

            # If we can't get a screenshot, return a placeholder
            return (
                f"Error during task execution: {str(e)}",
                Image(format="png", data=b""),
            )

    @server.tool(structured_output=False)
    async def run_multi_cua_tasks(
        ctx: Context, tasks: List[str], session_id: Optional[str] = None, concurrent: bool = False
    ) -> Any:
        """
        Run multiple Cua tasks and return a list of (combined text, screenshot).

        Args:
            tasks: List of task descriptions to execute
            session_id: Optional session ID for multi-client support. If not provided, a new session will be created.
            concurrent: If True, run tasks concurrently. If False, run sequentially (default).
        """
        total_tasks = len(tasks)
        if total_tasks == 0:
            ctx.report_progress(1.0)
            return []

        session_manager = get_session_manager()

        if concurrent and total_tasks > 1:
            # Run tasks concurrently
            logger.info(f"Running {total_tasks} tasks concurrently")
            ctx.info(f"Running {total_tasks} tasks concurrently")

            # Create tasks with progress tracking
            async def run_task_with_progress(
                task_index: int, task: str
            ) -> Tuple[int, Tuple[str, Image]]:
                ctx.report_progress(task_index / total_tasks)
                result = await run_cua_task(ctx, task, session_id)
                ctx.report_progress((task_index + 1) / total_tasks)
                return task_index, result

            # Create all task coroutines
            task_coroutines = [run_task_with_progress(i, task) for i, task in enumerate(tasks)]

            # Wait for all tasks to complete
            results_with_indices = await asyncio.gather(*task_coroutines, return_exceptions=True)

            # Sort results by original task order and handle exceptions
            results: List[Tuple[str, Image]] = []
            for result in results_with_indices:
                if isinstance(result, Exception):
                    logger.error(f"Task failed with exception: {result}")
                    ctx.error(f"Task failed: {str(result)}")
                    results.append((f"Task failed: {str(result)}", Image(format="png", data=b"")))
                else:
                    _, task_result = result
                    results.append(task_result)

            return results
        else:
            # Run tasks sequentially (original behavior)
            logger.info(f"Running {total_tasks} tasks sequentially")
            ctx.info(f"Running {total_tasks} tasks sequentially")

            results: List[Tuple[str, Image]] = []
            for i, task in enumerate(tasks):
                logger.info(f"Running task {i+1}/{total_tasks}: {task}")
                ctx.info(f"Running task {i+1}/{total_tasks}: {task}")

                ctx.report_progress(i / total_tasks)
                task_result = await run_cua_task(ctx, task, session_id)
                results.append(task_result)
                ctx.report_progress((i + 1) / total_tasks)

            return results

    @server.tool(structured_output=False)
    async def get_session_stats(ctx: Context) -> Dict[str, Any]:
        """
        Get statistics about active sessions and resource usage.
        """
        session_manager = get_session_manager()
        return session_manager.get_session_stats()

    @server.tool(structured_output=False)
    async def cleanup_session(ctx: Context, session_id: str) -> str:
        """
        Cleanup a specific session and release its resources.

        Args:
            session_id: The session ID to cleanup
        """
        session_manager = get_session_manager()
        await session_manager.cleanup_session(session_id)
        return f"Session {session_id} cleanup initiated"

    return server


server = serve()


async def run_server():
    """Run the MCP server with proper lifecycle management."""
    session_manager = None
    try:
        logger.debug("Starting MCP server...")

        # Initialize session manager
        session_manager = await initialize_session_manager()
        logger.info("Session manager initialized")

        # Set up signal handlers for graceful shutdown
        def signal_handler(signum, frame):
            logger.info(f"Received signal {signum}, initiating graceful shutdown...")
            # Create a task to shutdown gracefully
            asyncio.create_task(graceful_shutdown())

        signal.signal(signal.SIGINT, signal_handler)
        signal.signal(signal.SIGTERM, signal_handler)

        # Start the server
        logger.info("Starting FastMCP server...")
        # Use run_stdio_async directly instead of server.run() to avoid nested event loops
        await server.run_stdio_async()

    except Exception as e:
        logger.error(f"Error starting server: {e}")
        traceback.print_exc(file=sys.stderr)
        raise
    finally:
        # Ensure cleanup happens
        if session_manager:
            logger.info("Shutting down session manager...")
            await shutdown_session_manager()


async def graceful_shutdown():
    """Gracefully shutdown the server and all sessions."""
    logger.info("Initiating graceful shutdown...")
    try:
        await shutdown_session_manager()
        logger.info("Graceful shutdown completed")
    except Exception as e:
        logger.error(f"Error during graceful shutdown: {e}")
    finally:
        # Exit the process
        import os

        os._exit(0)


def main():
    """Run the MCP server with proper async lifecycle management."""
    try:
        # Use anyio.run instead of asyncio.run to avoid nested event loop issues
        anyio.run(run_server)
    except KeyboardInterrupt:
        logger.info("Server interrupted by user")
    except Exception as e:
        logger.error(f"Error starting server: {e}")
        traceback.print_exc(file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
