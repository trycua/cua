from fastapi import FastAPI, WebSocket, WebSocketDisconnect
from typing import List, Dict, Any
import uvicorn
import logging
import asyncio
import json
import traceback
from contextlib import redirect_stdout, redirect_stderr
from io import StringIO
from .handlers.factory import HandlerFactory

# Set up logging with more detail
logging.basicConfig(
    level=logging.DEBUG, format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger(__name__)

# Configure WebSocket with larger message size
WEBSOCKET_MAX_SIZE = 1024 * 1024 * 10  # 10MB limit

# Configure application with WebSocket settings
app = FastAPI(
    title="Computer API",
    description="API for the Computer project",
    version="0.1.0",
    websocket_max_size=WEBSOCKET_MAX_SIZE,
)


class ConnectionManager:
    def __init__(self):
        self.active_connections: List[WebSocket] = []
        # Create OS-specific handlers
        self.accessibility_handler, self.automation_handler = HandlerFactory.create_handlers()

    async def connect(self, websocket: WebSocket):
        await websocket.accept()
        self.active_connections.append(websocket)

    def disconnect(self, websocket: WebSocket):
        self.active_connections.remove(websocket)


manager = ConnectionManager()


@app.websocket("/ws", name="websocket_endpoint")
async def websocket_endpoint(websocket: WebSocket):
    # WebSocket message size is configured at the app or endpoint level, not on the instance
    await manager.connect(websocket)

    # Map commands to appropriate handler methods
    handlers = {
        # Accessibility commands
        "get_accessibility_tree": manager.accessibility_handler.get_accessibility_tree,
        "find_element": manager.accessibility_handler.find_element,
        # Automation commands
        "screenshot": manager.automation_handler.screenshot,
        "left_click": manager.automation_handler.left_click,
        "right_click": manager.automation_handler.right_click,
        "double_click": manager.automation_handler.double_click,
        "scroll_down": manager.automation_handler.scroll_down,
        "scroll_up": manager.automation_handler.scroll_up,
        "move_cursor": manager.automation_handler.move_cursor,
        "type_text": manager.automation_handler.type_text,
        "press_key": manager.automation_handler.press_key,
        "drag_to": manager.automation_handler.drag_to,
        "drag": manager.automation_handler.drag,
        "hotkey": manager.automation_handler.hotkey,
        "get_cursor_position": manager.automation_handler.get_cursor_position,
        "get_screen_size": manager.automation_handler.get_screen_size,
        "copy_to_clipboard": manager.automation_handler.copy_to_clipboard,
        "set_clipboard": manager.automation_handler.set_clipboard,
        "run_command": manager.automation_handler.run_command,
    }

    try:
        while True:
            try:
                data = await websocket.receive_json()
                command = data.get("command")
                params = data.get("params", {})

                if command not in handlers:
                    await websocket.send_json(
                        {"success": False, "error": f"Unknown command: {command}"}
                    )
                    continue

                try:
                    result = await handlers[command](**params)
                    await websocket.send_json({"success": True, **result})
                except Exception as cmd_error:
                    logger.error(f"Error executing command {command}: {str(cmd_error)}")
                    logger.error(traceback.format_exc())
                    await websocket.send_json({"success": False, "error": str(cmd_error)})

            except WebSocketDisconnect:
                raise
            except json.JSONDecodeError as json_err:
                logger.error(f"JSON decode error: {str(json_err)}")
                await websocket.send_json(
                    {"success": False, "error": f"Invalid JSON: {str(json_err)}"}
                )
            except Exception as loop_error:
                logger.error(f"Error in message loop: {str(loop_error)}")
                logger.error(traceback.format_exc())
                await websocket.send_json({"success": False, "error": str(loop_error)})

    except WebSocketDisconnect:
        logger.info("Client disconnected")
        manager.disconnect(websocket)
    except Exception as e:
        logger.error(f"Fatal error in websocket connection: {str(e)}")
        logger.error(traceback.format_exc())
        try:
            await websocket.close()
        except:
            pass
        manager.disconnect(websocket)


if __name__ == "__main__":
    uvicorn.run(app, host="0.0.0.0", port=8000)
