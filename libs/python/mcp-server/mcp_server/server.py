import asyncio
import base64
import inspect
import logging
import os
import sys
from tabnanny import verbose
import traceback
from typing import Any, Dict, List, Optional, Union, Tuple

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
    from computer import Computer
    from agent import ComputerAgent

    logger.debug("Successfully imported Computer and Agent modules")
except ImportError as e:
    logger.error(f"Failed to import Computer/Agent modules: {e}")
    traceback.print_exc(file=sys.stderr)
    sys.exit(1)

# Global computer instance for reuse
global_computer = None

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
            if isinstance(part, dict) and part.get("type") in {"output_text", "text"} and part.get("text"):
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
    async def screenshot_cua(ctx: Context) -> Any:
        """
        Take a screenshot of the current MacOS VM screen and return the image.
        """
        global global_computer
        if global_computer is None:
            global_computer = Computer(verbosity=logging.INFO)
            await global_computer.run()
        screenshot = await global_computer.interface.screenshot()
        # Returning Image object is fine when structured_output=False
        return Image(format="png", data=screenshot)

    @server.tool(structured_output=False)
    async def run_cua_task(ctx: Context, task: str) -> Any:
        """
        Run a Computer-Use Agent (CUA) task in a MacOS VM and return (combined text, final screenshot).
        """
        global global_computer
        try:
            logger.info(f"Starting CUA task: {task}")

            # Initialize computer if needed
            if global_computer is None:
                global_computer = Computer(verbosity=logging.INFO)
                await global_computer.run()

            # Get model name
            model_name = os.getenv("CUA_MODEL_NAME", "anthropic/claude-3-5-sonnet-20241022")
            logger.info(f"Using model: {model_name}")

            # Create agent with the new v0.4.x API
            agent = ComputerAgent(
                model=model_name,
                only_n_most_recent_images=int(os.getenv("CUA_MAX_IMAGES", "3")),
                verbosity=logging.INFO,
                tools=[global_computer],
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
                        tool_name = output.get("name") or output.get("action", {}).get("type")
                        tool_input = output.get("input") or output.get("arguments") or output.get("action")
                        if call_id:
                            await _maybe_call_ctx_method(
                                ctx,
                                "yield_tool_call",
                                name=tool_name,
                                call_id=call_id,
                                input=tool_input,
                            )

                    elif output_type in {"tool_result", "computer_call_output", "function_call_output"}:
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
                                is_error=output.get("status") == "failed" or output.get("is_error", False),
                            )

            logger.info("CUA task completed successfully")
            ctx.info("CUA task completed successfully")

            screenshot_image = Image(
                format="png",
                data=await global_computer.interface.screenshot(),
            )

            return (
                "\n".join(aggregated_messages).strip() or "Task completed with no text output.",
                screenshot_image,
            )

        except Exception as e:
            error_msg = f"Error running CUA task: {str(e)}\n{traceback.format_exc()}"
            logger.error(error_msg)
            ctx.error(error_msg)
            # Return tuple with error message and a screenshot if possible
            try:
                if global_computer is not None:
                    screenshot = await global_computer.interface.screenshot()
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
    async def run_multi_cua_tasks(ctx: Context, tasks: List[str]) -> Any:
        """
        Run multiple CUA tasks in sequence and return a list of (combined text, screenshot).
        """
        total_tasks = len(tasks)
        if total_tasks == 0:
            ctx.report_progress(1.0)
            return []

        results: List[Tuple[str, Image]] = []
        for i, task in enumerate(tasks):
            logger.info(f"Running task {i+1}/{total_tasks}: {task}")
            ctx.info(f"Running task {i+1}/{total_tasks}: {task}")

            ctx.report_progress(i / total_tasks)
            task_result = await run_cua_task(ctx, task)
            results.append(task_result)
            ctx.report_progress((i + 1) / total_tasks)

        return results

    return server


server = serve()

def main():
    """Run the MCP server."""
    try:
        logger.debug("Starting MCP server...")
        server.run()
    except Exception as e:
        logger.error(f"Error starting server: {e}")
        traceback.print_exc(file=sys.stderr)
        sys.exit(1)

if __name__ == "__main__":
    main()
