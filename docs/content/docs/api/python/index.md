---
title: CUA Python SDK Documentation
---

This is the auto-generated API reference for all CUA Python SDK packages.

## Core

Core functionality shared across Cua components.

## Agent

agent - Decorator-based Computer Use Agent with liteLLM integration

### agent.AgentResponse

alias of `ResponsesAPIResponse`

### _class_ agent.ComputerAgent(model, tools=None, custom_loop=None, only_n_most_recent_images=None, callbacks=None, instructions=None, verbosity=None, trajectory_dir=None, max_retries=3, screenshot_delay=0.5, use_prompt_caching=False, max_trajectory_budget=None, telemetry_enabled=True, trust_remote_code=False, api_key=None, api_base=None, \*\*additional_generation_kwargs)

Bases: `object`

Main agent class that automatically selects the appropriate agent loop
based on the model and executes tool calls.

- **Parameters:**
  - **model** (`str`)
  - **tools** (`Optional`[`List`[`Any`]])
  - **custom_loop** (`Optional`[`Callable`])
  - **only_n_most_recent_images** (`Optional`[`int`])
  - **callbacks** (`Optional`[`List`[`Any`]])
  - **instructions** (`Optional`[`str`])
  - **verbosity** (`Optional`[`int`])
  - **trajectory_dir** (`Union`[`str`, `Path`, `dict`, `None`])
  - **max_retries** (`Optional`[`int`])
  - **screenshot_delay** (`Union`[`float`, `int`, `None`])
  - **use_prompt_caching** (`Optional`[`bool`])
  - **max_trajectory_budget** (`Union`[`float`, `dict`, `None`])
  - **telemetry_enabled** (`Optional`[`bool`])
  - **trust_remote_code** (`Optional`[`bool`])
  - **api_key** (`Optional`[`str`])
  - **api_base** (`Optional`[`str`])

#### get_capabilities()

Get list of capabilities supported by the current agent config.

- **Return type:**
  `List`[`Literal`[`'step'`, `'click'`]]
- **Returns:**
  List of capability strings (e.g., [“step”, “click”])

#### open(port=None)

Start the playground server and open it in the browser.

This method starts a local HTTP server that exposes the /responses endpoint
and automatically opens the CUA playground interface in the default browser.

- **Parameters:**
  **port** (`Optional`[`int`]) – Port to run the server on. If None, finds an available port automatically.

### Example

```python
>>> agent = ComputerAgent(model="claude-sonnet-4")
>>> agent.open()  # Starts server and opens browser
```

#### _async_ predict_click(instruction, image_b64=None)

Predict click coordinates based on image and instruction.

- **Parameters:**
  - **instruction** (`str`) – Instruction for where to click
  - **image_b64** (`Optional`[`str`]) – Base64 encoded image (optional, will take screenshot if not provided)
- **Return type:**
  `Optional`[`Tuple`[`int`, `int`]]
- **Returns:**
  None or tuple with (x, y) coordinates

#### _async_ run(messages, stream=False, api_key=None, api_base=None, \*\*additional_generation_kwargs)

Run the agent with the given messages using Computer protocol handler pattern.

- **Parameters:**
  - **messages** (`Union`[`str`, `List`[`Union`[`EasyInputMessageParam`, `Message`, `ResponseOutputMessageParam`, `ResponseFileSearchToolCallParam`, `ResponseComputerToolCallParam`, `ComputerCallOutput`, `ResponseFunctionWebSearchParam`, `ResponseFunctionToolCallParam`, `FunctionCallOutput`, `ResponseReasoningItemParam`, `ImageGenerationCall`, `ResponseCodeInterpreterToolCallParam`, `LocalShellCall`, `LocalShellCallOutput`, `McpListTools`, `McpApprovalRequest`, `McpApprovalResponse`, `McpCall`, `ResponseCustomToolCallOutputParam`, `ResponseCustomToolCallParam`, `ItemReference`]], `List`[`Dict`[`str`, `Any`]]]) – List of message dictionaries
  - **stream** (`bool`) – Whether to stream the response
  - **api_key** (`Optional`[`str`]) – Optional API key override for the model provider
  - **api_base** (`Optional`[`str`]) – Optional API base URL override for the model provider
  - **\*\*additional_generation_kwargs** – Additional arguments passed to the model provider
- **Return type:**
  `AsyncGenerator`[`Dict`[`str`, `Any`], `None`]
- **Returns:**
  AsyncGenerator that yields response chunks

### agent.register_agent(models, priority=0)

Decorator to register an AsyncAgentConfig class.

- **Parameters:**
  - **models** (`str`) – Regex pattern to match supported models
  - **priority** (`int`) – Priority for agent selection (higher = more priority)

## Computer

CUA Computer Interface for cross-platform computer control.

### _class_ computer.Computer(display='1024x768', memory='8GB', cpu='4', os_type='macos', name='', image=None, shared_directories=None, use_host_computer_server=False, verbosity=20, telemetry_enabled=True, provider_type=VMProviderType.LUME, provider_port=7777, noVNC_port=8006, api_port=None, host='localhost', api_host=None, storage=None, ephemeral=False, api_key=None, experiments=None, timeout=100, run_opts=None)

Bases: `object`

Computer is the main class for interacting with the computer.

- **Parameters:**
  - **display** (`Union`[`Display`, `Dict`[`str`, `int`], `str`])
  - **memory** (`str`)
  - **cpu** (`str`)
  - **os_type** (`Literal`[`'macos'`, `'linux'`, `'windows'`, `'android'`])
  - **name** (`str`)
  - **image** (`Optional`[`str`])
  - **shared_directories** (`Optional`[`List`[`str`]])
  - **use_host_computer_server** (`bool`)
  - **verbosity** (`Union`[`int`, `LogLevel`])
  - **telemetry_enabled** (`bool`)
  - **provider_type** (`Union``str`, [`VMProviderType`])
  - **provider_port** (`Optional`[`int`])
  - **noVNC_port** (`Optional`[`int`])
  - **api_port** (`Optional`[`int`])
  - **host** (`str`)
  - **api_host** (`Optional`[`str`])
  - **storage** (`Optional`[`str`])
  - **ephemeral** (`bool`)
  - **api_key** (`Optional`[`str`])
  - **experiments** (`Optional`[`List`[`str`]])
  - **timeout** (`int`)
  - **run_opts** (`Optional`[`Dict`[`str`, `Any`]])

#### create_desktop_from_apps(apps)

Create a virtual desktop from a list of app names, returning a DioramaComputer
that proxies Diorama.Interface but uses diorama_cmds via the computer interface.

- **Parameters:**
  **apps** (_list_ _[\*\*str_ _]_) – List of application names to include in the desktop.
- **Returns:**
  A proxy object with the Diorama interface, but using diorama_cmds.
- **Return type:**
  DioramaComputer

#### _async_ disconnect()

Disconnect from the computer’s WebSocket interface.

- **Return type:**
  `None`

#### _async_ get_ip(max_retries=15, retry_delay=3)

Get the IP address of the VM or localhost if using host computer server.

This method delegates to the provider’s get_ip method, which waits indefinitely
until the VM has a valid IP address.

- **Parameters:**
  - **max_retries** (`int`) – Unused parameter, kept for backward compatibility
  - **retry_delay** (`int`) – Delay between retries in seconds (default: 2)
- **Return type:**
  `str`
- **Returns:**
  IP address of the VM or localhost if using host computer server

#### get_screenshot_size(screenshot)

Get the dimensions of a screenshot.

- **Parameters:**
  **screenshot** (`bytes`) – The screenshot bytes
- **Returns:**
  Dictionary containing ‘width’ and ‘height’ of the image
- **Return type:**
  Dict[str, int]

#### _property_ interface

Get the computer interface for interacting with the VM.

- **Returns:**
  The computer interface (wrapped with tracing if tracing is active)

#### _async_ pip_install(requirements)

Install packages using the system Python/pip (no venv).

- **Parameters:**
  **requirements** (`list`[`str`]) – List of package requirements to install globally/user site.
- **Returns:**
  Tuple of (stdout, stderr) from the installation command

#### _async_ playwright_exec(command, params=None)

Execute a Playwright browser command.

- **Parameters:**
  - **command** (`str`) – The browser command to execute (visit_url, click, type, scroll, web_search)
  - **params** (`Optional`[`Dict`]) – Command parameters
- **Return type:**
  `Dict`[`str`, `Any`]
- **Returns:**
  Dict containing the command result

### Examples

# Navigate to a URL

await computer.playwright_exec(“visit_url”, {“url”: “https://example.com”})

# Click at coordinates

await computer.playwright_exec(“click”, {“x”: 100, “y”: 200})

# Type text

await computer.playwright_exec(“type”, {“text”: “Hello, world!”})

# Scroll

await computer.playwright_exec(“scroll”, {“delta_x”: 0, “delta_y”: -100})

# Web search

await computer.playwright_exec(“web_search”, {“query”: “computer use agent”})

#### python_command(requirements=None, , venv_name='default', use_system_python=False, background=False)

Decorator to execute a Python function remotely in this Computer’s venv.

This mirrors computer.helpers.sandboxed() but binds to this instance and
optionally ensures required packages are installed before execution.

- **Parameters:**
  - **requirements** (`Optional`[`List`[`str`]]) – Packages to install in the virtual environment.
  - **venv_name** (`str`) – Name of the virtual environment to use.
  - **use_system_python** (`bool`) – If True, use the system Python/pip instead of a venv.
  - **background** (`bool`) – If True, run the function detached and return the child PID immediately.
- **Return type:**
  `Callable`[[`Callable`[[`ParamSpec`(`P`, bound= `None`)], `TypeVar`(`R`)]], `Callable`[[`ParamSpec`(`P`, bound= `None`)], `Awaitable`[`TypeVar`(`R`)]]]
- **Returns:**
  A decorator that turns a local function into an async callable which
  runs remotely and returns the function’s result.

#### _async_ python_exec(python_func, \*args, \*\*kwargs)

Execute a Python function using the system Python (no venv).

Uses source extraction and base64 transport, mirroring venv_exec but
without virtual environment activation.

Returns the function result or raises a reconstructed exception with
remote traceback context appended.

#### _async_ python_exec_background(python_func, \*args, requirements=None, \*\*kwargs)

Run a Python function with the system interpreter in the background and return PID.

Uses a short launcher Python that spawns a detached child and exits immediately.

- **Parameters:**
  **requirements** (`Optional`[`List`[`str`]])
- **Return type:**
  `int`

#### _async_ restart()

Restart the computer.

If using a VM provider that supports restart, this will issue a restart
without tearing down the provider context, then reconnect the interface.
Falls back to stop()+run() when a provider restart is not available.

- **Return type:**
  `None`

#### _async_ run()

Initialize the VM and computer interface.

- **Return type:**
  `Optional`[`str`]

#### _async_ start()

Start the computer.

- **Return type:**
  `None`

#### _async_ stop()

Disconnect from the computer’s WebSocket interface and stop the computer.

- **Return type:**
  `None`

#### _property_ telemetry*enabled *: bool\_

Check if telemetry is enabled for this computer instance.

- **Returns:**
  True if telemetry is enabled, False otherwise
- **Return type:**
  bool

#### _async_ to_screen_coordinates(x, y)

Convert normalized coordinates to screen coordinates.

- **Parameters:**
  - **x** (`float`) – X coordinate between 0 and 1
  - **y** (`float`) – Y coordinate between 0 and 1
- **Returns:**
  Screen coordinates (x, y)
- **Return type:**
  tuple[float, float]

#### _async_ to_screenshot_coordinates(x, y)

Convert screen coordinates to screenshot coordinates.

- **Parameters:**
  - **x** (`float`) – X coordinate in screen space
  - **y** (`float`) – Y coordinate in screen space
- **Returns:**
  (x, y) coordinates in screenshot space
- **Return type:**
  tuple[float, float]

#### _property_ tracing _: ComputerTracing_

Get the computer tracing instance for recording sessions.

- **Returns:**
  The tracing instance
- **Return type:**
  ComputerTracing

#### _async_ update(cpu=None, memory=None)

Update VM settings.

- **Parameters:**
  - **cpu** (`Optional`[`int`])
  - **memory** (`Optional`[`str`])

#### _async_ venv_cmd(venv_name, command)

Execute a shell command in a virtual environment.

- **Parameters:**
  - **venv_name** (`str`) – Name of the virtual environment
  - **command** (`str`) – Shell command to execute in the virtual environment
- **Returns:**
  Tuple of (stdout, stderr) from the command execution

#### _async_ venv_exec(venv_name, python_func, \*args, \*\*kwargs)

Execute Python function in a virtual environment using source code extraction.

- **Parameters:**
  - **venv_name** (`str`) – Name of the virtual environment
  - **python_func** – A callable function to execute
  - **\*args** – Positional arguments to pass to the function
  - **\*\*kwargs** – Keyword arguments to pass to the function
- **Returns:**
  The result of the function execution, or raises any exception that occurred

#### _async_ venv_exec_background(venv_name, python_func, \*args, requirements=None, \*\*kwargs)

Run the Python function in the venv in the background and return the PID.

Uses a short launcher Python that spawns a detached child and exits immediately.

- **Parameters:**
  - **venv_name** (`str`)
  - **requirements** (`Optional`[`List`[`str`]])
- **Return type:**
  `int`

#### _async_ venv_install(venv_name, requirements)

Install packages in a virtual environment.

- **Parameters:**
  - **venv_name** (`str`) – Name of the virtual environment
  - **requirements** (`list`[`str`]) – List of package requirements to install
- **Returns:**
  Tuple of (stdout, stderr) from the installation command

#### _async_ wait_vm_ready()

Wait for VM to be ready with an IP address.

- **Return type:**
  `Optional`[`Dict`[`str`, `Any`]]
- **Returns:**
  VM status information or None if using host computer server.

### _class_ computer.VMProviderType(\*values)

Bases: `StrEnum`

Enum of supported VM provider types.

#### CLOUD _= 'cloud'_

#### DOCKER _= 'docker'_

#### LUME _= 'lume'_

#### LUMIER _= 'lumier'_

#### UNKNOWN _= 'unknown'_

#### WINSANDBOX _= 'winsandbox'_

## Computer Server

Computer API package.
Provides a server interface for the Computer API.

### _class_ computer_server.Server(host='0.0.0.0', port=8000, log_level='info', ssl_keyfile=None, ssl_certfile=None)

Bases: `object`

Server interface for Computer API.

Usage:
: from computer_api import Server
<br/>

# Synchronous usage

server = Server()
server.start() # Blocks until server is stopped
<br/>

# Asynchronous usage

server = Server()
await server.start_async() # Starts server in background

# Do other things

await server.stop() # Stop the server

- **Parameters:**
  - **host** (`str`)
  - **port** (`int`)
  - **log_level** (`str`)
  - **ssl_keyfile** (`Optional`[`str`])
  - **ssl_certfile** (`Optional`[`str`])

#### start()

Start the server synchronously. This will block until the server is stopped.

- **Return type:**
  `None`

#### _async_ start_async()

Start the server asynchronously. This will return immediately and the server
will run in the background.

- **Return type:**
  `None`

#### _async_ stop()

Stop the server if it’s running asynchronously.

- **Return type:**
  `None`

### computer_server.run_cli()

Entry point for CLI

- **Return type:**
  `None`

## SOM

SOM - Computer Vision and OCR library for detecting and analyzing UI elements.

### _class_ som.BoundingBox(\*\*data)

Bases: `BaseModel`

Normalized bounding box coordinates.

- **Parameters:**
  **data** (`Any`)

#### _property_ coordinates _: List[float]_

Get coordinates as a list [x1, y1, x2, y2].

#### model*config *: ClassVar[ConfigDict]\_ _= {}_

Configuration for the model, should be a dictionary conforming to [ConfigDict][pydantic.config.ConfigDict].

#### x1 _: `float`_

#### x2 _: `float`_

#### y1 _: `float`_

#### y2 _: `float`_

### _class_ som.IconElement(\*\*data)

Bases: `UIElement`

An interactive icon element.

- **Parameters:**
  **data** (`Any`)

#### interactivity _: `bool`_

#### model*config *: ClassVar[ConfigDict]\_ _= {}_

Configuration for the model, should be a dictionary conforming to [ConfigDict][pydantic.config.ConfigDict].

#### scale _: `Optional`[`int`]_

#### type _: `Literal`[`'icon'`]_

### _class_ som.OmniParser(model_path=None, cache_dir=None, force_device=None)

Bases: `object`

Enhanced UI parser using computer vision and OCR for detecting interactive elements.

- **Parameters:**
  - **model_path** (`Union`[`str`, `Path`, `None`])
  - **cache_dir** (`Union`[`str`, `Path`, `None`])
  - **force_device** (`Optional`[`str`])

#### parse(screenshot_data, box_threshold=0.3, iou_threshold=0.1, use_ocr=True)

Parse a UI screenshot to detect interactive elements and text.

- **Parameters:**
  - **screenshot_data** (`Union`[`bytes`, `str`]) – Raw bytes or base64 string of the screenshot
  - **box_threshold** (`float`) – Confidence threshold for detection
  - **iou_threshold** (`float`) – IOU threshold for NMS
  - **use_ocr** (`bool`) – Whether to enable OCR processing
- **Return type:**
  `ParseResult`
- **Returns:**
  ParseResult object containing elements, annotated image, and metadata

#### process_image(image, box_threshold=0.3, iou_threshold=0.1, use_ocr=True)

Process an image to detect UI elements and optionally text.

- **Parameters:**
  - **image** (`Image`) – Input PIL Image
  - **box_threshold** (`float`) – Confidence threshold for detection
  - **iou_threshold** (`float`) – IOU threshold for NMS
  - **use_ocr** (`bool`) – Whether to enable OCR processing
- **Return type:**
  `Tuple``Image`, `List`[[`UIElement`]]
- **Returns:**
  Tuple of (annotated image, list of detections)

### _class_ som.ParseResult(\*\*data)

Bases: `BaseModel`

Result of parsing a UI screenshot.

- **Parameters:**
  **data** (`Any`)

#### annotated*image_base64 *: `str`\_

#### elements _: `List`[`UIElement`]_

#### _property_ height _: int_

Get image height from metadata.

#### _property_ image _: ImageData_

Get image data as a convenience property.

#### metadata _: `ParserMetadata`_

#### model*config *: ClassVar[ConfigDict]\_ _= {}_

Configuration for the model, should be a dictionary conforming to [ConfigDict][pydantic.config.ConfigDict].

#### model_dump()

Convert model to dict for compatibility with older code.

- **Return type:**
  `Dict`[`str`, `Any`]

#### parsed*content_list *: `Optional`[`List`[`Dict`[`str`, `Any`]]]\_

#### screen*info *: `Optional`[`List`[`str`]]\_

#### _property_ width _: int_

Get image width from metadata.

### _class_ som.ParserMetadata(\*\*data)

Bases: `BaseModel`

Metadata about the parsing process.

- **Parameters:**
  **data** (`Any`)

#### device _: `str`_

#### _property_ height _: int_

Get image height from image_size.

#### image*size *: `Tuple`[`int`, `int`]\_

#### latency _: `float`_

#### model*config *: ClassVar[ConfigDict]\_ _= {}_

Configuration for the model, should be a dictionary conforming to [ConfigDict][pydantic.config.ConfigDict].

#### num*icons *: `int`\_

#### num*text *: `int`\_

#### ocr*enabled *: `bool`\_

#### _property_ width _: int_

Get image width from image_size.

### _class_ som.TextElement(\*\*data)

Bases: `UIElement`

A text element.

- **Parameters:**
  **data** (`Any`)

#### content _: `str`_

#### interactivity _: `bool`_

#### model*config *: ClassVar[ConfigDict]\_ _= {}_

Configuration for the model, should be a dictionary conforming to [ConfigDict][pydantic.config.ConfigDict].

#### type _: `Literal`[`'text'`]_

### _class_ som.UIElement(\*\*data)

Bases: `BaseModel`

Base class for UI elements.

- **Parameters:**
  **data** (`Any`)

#### bbox _: `BoundingBox`_

#### confidence _: `float`_

#### id _: `Optional`[`int`]_

#### interactivity _: `bool`_

#### model*config *: ClassVar[ConfigDict]\_ _= {}_

Configuration for the model, should be a dictionary conforming to [ConfigDict][pydantic.config.ConfigDict].

#### type _: `Literal`[`'icon'`, `'text'`]_

## MCP Server

MCP Server for Computer-Use Agent (CUA).

### mcp_server.main()

Run the MCP server with proper async lifecycle management.

## Bench UI
