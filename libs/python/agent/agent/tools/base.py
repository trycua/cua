"""
Base tool interface and registration system for agent tools.
Provides a protocol for implementing tools that can be registered and discovered.
"""

import json
from abc import ABC, abstractmethod
from typing import Dict, List, Optional, Union

# Global tool registry
TOOL_REGISTRY: Dict[str, type] = {}


def register_tool(name: str, allow_overwrite: bool = False):
    """
    Decorator to register a tool class with a given name.

    Args:
        name: The name to register the tool under
        allow_overwrite: Whether to allow overwriting an existing tool with the same name

    Returns:
        Decorator function that registers the class

    Example:
        @register_tool("my_tool")
        class MyTool(BaseTool):
            ...
    """

    def decorator(cls):
        if name in TOOL_REGISTRY:
            if allow_overwrite:
                print(f"Warning: Tool `{name}` already exists! Overwriting with class {cls}.")
            else:
                raise ValueError(
                    f"Tool `{name}` already exists! Please ensure that the tool name is unique."
                )
        if hasattr(cls, "name") and cls.name and (cls.name != name):
            raise ValueError(
                f'{cls.__name__}.name="{cls.name}" conflicts with @register_tool(name="{name}").'
            )
        cls.name = name
        TOOL_REGISTRY[name] = cls
        return cls

    return decorator


def is_tool_schema(obj: dict) -> bool:
    """
    Check if obj is a valid JSON schema describing a tool compatible with OpenAI's tool calling.

    Example valid schema:
    {
      "name": "get_current_weather",
      "description": "Get the current weather in a given location",
      "parameters": {
        "type": "object",
        "properties": {
          "location": {
            "type": "string",
            "description": "The city and state, e.g. San Francisco, CA"
          },
          "unit": {
            "type": "string",
            "enum": ["celsius", "fahrenheit"]
          }
        },
        "required": ["location"]
      }
    }
    """
    try:
        # Basic structure validation
        assert set(obj.keys()) == {"name", "description", "parameters"}
        assert isinstance(obj["name"], str)
        assert obj["name"].strip()
        assert isinstance(obj["description"], str)
        assert isinstance(obj["parameters"], dict)

        # Parameters structure validation
        assert "type" in obj["parameters"]
        assert obj["parameters"]["type"] == "object"
        assert "properties" in obj["parameters"]
        assert isinstance(obj["parameters"]["properties"], dict)

        if "required" in obj["parameters"]:
            assert isinstance(obj["parameters"]["required"], list)
            assert set(obj["parameters"]["required"]).issubset(
                set(obj["parameters"]["properties"].keys())
            )

        return True
    except (AssertionError, KeyError, TypeError):
        return False


class BaseTool(ABC):
    """
    Base class for all agent tools.

    Tools must implement:
    - name: str - The tool name (set by @register_tool decorator)
    - description: property that returns str - Tool description
    - parameters: property that returns dict - JSON schema for tool parameters
    - call: method - Execute the tool with given parameters
    """

    name: str = ""

    def __init__(self, cfg: Optional[dict] = None):
        """
        Initialize the tool.

        Args:
            cfg: Optional configuration dictionary
        """
        self.cfg = cfg or {}

        if not self.name:
            raise ValueError(
                f"You must set {self.__class__.__name__}.name, either by "
                f"@register_tool(name=...) or explicitly setting "
                f"{self.__class__.__name__}.name"
            )

        # Validate schema if parameters is a dict
        if isinstance(self.parameters, dict):
            if not is_tool_schema(
                {"name": self.name, "description": self.description, "parameters": self.parameters}
            ):
                raise ValueError(
                    "The parameters, when provided as a dict, must conform to a "
                    "valid openai-compatible JSON schema."
                )

    @property
    @abstractmethod
    def description(self) -> str:
        """Return the tool description."""
        raise NotImplementedError

    @property
    @abstractmethod
    def parameters(self) -> dict:
        """Return the JSON schema for tool parameters."""
        raise NotImplementedError

    @abstractmethod
    def call(self, params: Union[str, dict], **kwargs) -> Union[str, list, dict]:
        """
        Execute the tool with the given parameters.

        Args:
            params: The parameters for the tool call (JSON string or dict)
            **kwargs: Additional keyword arguments

        Returns:
            The result of the tool execution
        """
        raise NotImplementedError

    def _verify_json_format_args(self, params: Union[str, dict]) -> dict:
        """
        Verify and parse the parameters as JSON.

        Args:
            params: Parameters as string or dict

        Returns:
            Parsed parameters as dict

        Raises:
            ValueError: If parameters are not valid JSON or don't match schema
        """
        if isinstance(params, str):
            try:
                params_json: dict = json.loads(params)
            except json.JSONDecodeError as e:
                raise ValueError(f"Parameters must be formatted as valid JSON: {e}")
        else:
            params_json: dict = params

        # Validate against schema if using dict parameters
        if isinstance(self.parameters, dict):
            try:
                # Basic validation of required fields
                if "required" in self.parameters:
                    for field in self.parameters["required"]:
                        if field not in params_json:
                            raise ValueError(f'Required parameter "{field}" is missing')
            except (KeyError, TypeError) as e:
                raise ValueError(f"Invalid parameters: {e}")

        return params_json

    @property
    def function(self) -> dict:
        """
        Return the function information for this tool.

        Returns:
            Dict with tool metadata
        """
        return {
            "name": self.name,
            "description": self.description,
            "parameters": self.parameters,
        }


def get_registered_tools() -> Dict[str, type]:
    """
    Get all registered tools.

    Returns:
        Dictionary mapping tool names to tool classes
    """
    return TOOL_REGISTRY.copy()


def get_tool(name: str) -> Optional[type]:
    """
    Get a registered tool by name.

    Args:
        name: The tool name

    Returns:
        The tool class, or None if not found
    """
    return TOOL_REGISTRY.get(name)


class BaseComputerTool(BaseTool):
    """
    Base class for computer tools that can provide screenshots.

    Computer tools must implement:
    - All BaseTool requirements (name, description, parameters, call)
    - screenshot() method that returns screenshot as base64 string
    """

    @abstractmethod
    async def screenshot(self) -> str:
        """
        Take a screenshot of the computer/browser.

        Returns:
            Screenshot image data as base64-encoded string
        """
        raise NotImplementedError
