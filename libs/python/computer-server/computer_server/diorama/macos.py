import inspect
import platform
import sys
from typing import Optional

from computer_server.diorama.base import BaseDioramaHandler
from computer_server.diorama.diorama import Diorama


class MacOSDioramaHandler(BaseDioramaHandler):
    """Handler for Diorama commands on macOS, using local diorama module."""

    async def diorama_cmd(self, action: str, arguments: Optional[dict] = None) -> dict:
        if platform.system().lower() != "darwin":
            return {"success": False, "error": "Diorama is only supported on macOS."}
        try:
            app_list = arguments.get("app_list") if arguments else None
            if not app_list:
                return {"success": False, "error": "Missing 'app_list' in arguments"}
            diorama = Diorama(app_list)
            interface = diorama.interface
            if not hasattr(interface, action):
                return {"success": False, "error": f"Unknown diorama action: {action}"}
            method = getattr(interface, action)
            # Remove app_list from arguments before calling the method
            filtered_arguments = dict(arguments)
            filtered_arguments.pop("app_list", None)
            if inspect.iscoroutinefunction(method):
                result = await method(**(filtered_arguments or {}))
            else:
                result = method(**(filtered_arguments or {}))
            return {"success": True, "result": result}
        except Exception as e:
            import traceback

            return {"success": False, "error": str(e), "trace": traceback.format_exc()}
