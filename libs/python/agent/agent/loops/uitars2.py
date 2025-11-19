"""
UITARS-2 agent loop implementation using LiteLLM.
- Prepends a system prompt modeled after the training prompts in examples/seed_16_gui.ipynb
- Converts Responses items -> completion messages
- Calls litellm.acompletion
- Parses <seed:tool_call> ... </seed:tool_call> outputs back into Responses items (computer actions)
"""

from __future__ import annotations

import base64
import io
import json
import re
from typing import Any, Dict, List, Optional, Tuple

import litellm
from litellm.responses.litellm_completion_transformation.transformation import (
    LiteLLMCompletionResponsesConfig,
)

from ..decorators import register_agent
from .omniparser import get_last_computer_call_output  # type: ignore

try:
    from PIL import Image  # type: ignore
except Exception:  # pragma: no cover
    Image = None  # type: ignore
from ..responses import (
    convert_responses_items_to_completion_messages,
    make_click_item,
    make_double_click_item,
    make_drag_item,
    make_function_call_item,
    make_keypress_item,
    make_move_item,
    make_output_text_item,
    make_reasoning_item,
    make_screenshot_item,
    make_scroll_item,
    make_type_item,
    make_wait_item,
)
from ..types import AgentCapability

TOOL_SCHEMAS: List[Dict[str, Any]] = [
    {
        "type": "function",
        "name": "open_computer",
        "parameters": {},
        "description": "Open computer.",
    },
    {
        "type": "function",
        "name": "click",
        "parameters": {
            "type": "object",
            "properties": {
                "point": {
                    "type": "string",
                    "description": "Click coordinates. The format is: <point>x y</point>",
                }
            },
            "required": ["point"],
        },
        "description": "Mouse left single click action.",
    },
    {
        "type": "function",
        "name": "left_double",
        "parameters": {
            "type": "object",
            "properties": {
                "point": {
                    "type": "string",
                    "description": "Click coordinates. The format is: <point>x y</point>",
                }
            },
            "required": ["point"],
        },
        "description": "Mouse left double click action.",
    },
    {
        "type": "function",
        "name": "right_single",
        "parameters": {
            "type": "object",
            "properties": {
                "point": {
                    "type": "string",
                    "description": "Click coordinates. The format is: <point>x y</point>",
                }
            },
            "required": ["point"],
        },
        "description": "Mouse right single click action.",
    },
    {
        "type": "function",
        "name": "scroll",
        "parameters": {
            "type": "object",
            "properties": {
                "point": {
                    "type": "string",
                    "description": "Scroll start position. If not specified, default to execute on the current mouse position. The format is: <point>x y</point>",
                },
                "direction": {
                    "type": "string",
                    "description": "Scroll direction.",
                    "enum": ["up", "down", "left", "right"],
                },
            },
            "required": ["direction"],
        },
        "description": "Scroll action.",
    },
    {
        "type": "function",
        "name": "move_to",
        "parameters": {
            "type": "object",
            "properties": {
                "point": {
                    "type": "string",
                    "description": "Target coordinates. The format is: <point>x y</point>",
                }
            },
            "required": ["point"],
        },
        "description": "Mouse move action.",
    },
    {
        "type": "function",
        "name": "hotkey",
        "parameters": {
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Hotkeys you want to press. Split keys with a space and use lowercase.",
                }
            },
            "required": ["key"],
        },
        "description": "Press hotkey.",
    },
    {
        "type": "function",
        "name": "finished",
        "parameters": {
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Provide the final answer or response to complete the task.",
                }
            },
            "required": [],
        },
        "description": "This function is used to indicate the completion of a task by providing the final answer or response.",
    },
    {
        "type": "function",
        "name": "press",
        "parameters": {
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Key you want to press. Only one key can be pressed at one time.",
                }
            },
            "required": ["key"],
        },
        "description": "Press key.",
    },
    {
        "type": "function",
        "name": "release",
        "parameters": {
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Key you want to release. Only one key can be released at one time.",
                }
            },
            "required": ["key"],
        },
        "description": "Release key.",
    },
    {
        "type": "function",
        "name": "mouse_down",
        "parameters": {
            "type": "object",
            "properties": {
                "point": {
                    "type": "string",
                    "description": "Mouse down position. If not specified, default to execute on the current mouse position. The format is: <point>x y</point>",
                },
                "button": {
                    "type": "string",
                    "description": "Down button. Default to left.",
                    "enum": ["left", "right"],
                },
            },
            "required": [],
        },
        "description": "Mouse down action.",
    },
    {
        "type": "function",
        "name": "mouse_up",
        "parameters": {
            "type": "object",
            "properties": {
                "point": {
                    "type": "string",
                    "description": "Mouse up position. If not specified, default to execute on the current mouse position. The format is: <point>x y</point>",
                },
                "button": {
                    "type": "string",
                    "description": "Up button. Default to left.",
                    "enum": ["left", "right"],
                },
            },
            "required": [],
        },
        "description": "Mouse up action.",
    },
    {
        "type": "function",
        "name": "call_user",
        "parameters": {
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Message or information displayed to the user to request their input, feedback, or guidance.",
                }
            },
            "required": [],
        },
        "description": "This function is used to interact with the user by displaying a message and requesting their input, feedback, or guidance.",
    },
    {
        "type": "function",
        "name": "wait",
        "parameters": {
            "type": "object",
            "properties": {"time": {"type": "integer", "description": "Wait time in seconds."}},
            "required": [],
        },
        "description": "Wait for a while.",
    },
    {
        "type": "function",
        "name": "drag",
        "parameters": {
            "type": "object",
            "properties": {
                "start_point": {
                    "type": "string",
                    "description": "Drag start point. The format is: <point>x y</point>",
                },
                "end_point": {
                    "type": "string",
                    "description": "Drag end point. The format is: <point>x y</point>",
                },
            },
            "required": ["start_point", "end_point"],
        },
        "description": "Mouse left button drag action.",
    },
    {
        "type": "function",
        "name": "type",
        "parameters": {
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Type content. If you want to submit your input, use \\n at the end of content.",
                }
            },
            "required": ["content"],
        },
        "description": "Type content.",
    },
    {
        "type": "function",
        "name": "take_screenshot",
        "parameters": {},
        "description": "Take screenshot.",
    },
]


def _format_tool_schemas_json_lines(schemas: List[Dict[str, Any]]) -> str:
    # Nicely formatted: pretty JSON with indentation, separated by blank lines
    return "\n\n".join(json.dumps(s, ensure_ascii=False, indent=2) for s in schemas) + "\n\n"


_PROMPT_PREFIX = (
    "You should begin by detailing the internal reasoning process, and then present the answer to the user. "
    "The reasoning process should be enclosed within <think_never_used_51bce0c785ca2f68081bfa7d91973934> "
    "</think_never_used_51bce0c785ca2f68081bfa7d91973934> tags, as follows:\n"
    "<think_never_used_51bce0c785ca2f68081bfa7d91973934> reasoning process here "
    "</think_never_used_51bce0c785ca2f68081bfa7d91973934> answer here.\n\n"
    "You have different modes of thinking:\n"
    "Unrestricted think mode: Engage in an internal thinking process with thorough reasoning and reflections. "
    "You have an unlimited budget for thinking tokens and can continue thinking until you fully solve the problem.\n"
    "Efficient think mode: Provide a concise internal thinking process with efficient reasoning and reflections. "
    "You don't have a strict token budget but be less verbose and more direct in your thinking.\n"
    "No think mode: Respond directly to the question without any internal reasoning process or extra thinking tokens. "
    "Still follow the template with the minimum required thinking tokens to justify the answer.\n"
    "Budgeted think mode: Limit your internal reasoning and reflections to stay within the specified token budget\n\n"
    "Based on the complexity of the problem, select the appropriate mode for reasoning among the provided options listed below.\n\n"
    "Provided Mode(s):\nEfficient think.\n\n"
    "You are provided with a task description, a history of previous actions, and corresponding screenshots. "
    "Your goal is to perform the next action to complete the task. "
    "If performing the same action multiple times results in a static screen with no changes, attempt a modified or alternative action.\n\n"
    "## Function Definition\n\n"
    "- You have access to the following functions:\n\n"
)

_PROMPT_SUFFIX = (
    "- To call a function, use the following structure without any suffix:\n\n"
    "<gui_think> reasoning process </gui_think>\n"
    "<seed:tool_call><function=example_function_name><parameter=example_parameter_1>value_1</parameter>"
    "<parameter=example_parameter_2>multiline...\n</parameter></function></seed:tool_call>\n\n"
    "## Important Notes\n"
    "- Function calls must begin with <function= and end with </function>.\n"
    "- All required parameters must be explicitly provided.\n"
    "\n## Additional Notes\n"
    "- You can execute multiple actions within a single tool call. For example:\n"
    "<seed:tool_call><function=example_function_1><parameter=example_parameter_1>value_1</parameter><parameter=example_parameter_2>\n"
    "This is the value for the second parameter\nthat can span\nmultiple lines\n"
    "</parameter></function><function=example_function_2><parameter=example_parameter_3>value_4</parameter></function></seed:tool_call>"
)


SYSTEM_PROMPT = _PROMPT_PREFIX + _format_tool_schemas_json_lines(TOOL_SCHEMAS) + _PROMPT_SUFFIX


def _extract_function_schemas_from_tools(
    tools: Optional[List[Dict[str, Any]]],
) -> List[Dict[str, Any]]:
    schemas: List[Dict[str, Any]] = []
    if not tools:
        return schemas
    for t in tools:
        if t.get("type") == "function":
            fn = t.get("function", {})
            name = fn.get("name")
            params = fn.get("parameters", {})
            desc = fn.get("description", "")
            if name:
                schemas.append(
                    {
                        "type": "function",
                        "name": name,
                        "parameters": params if isinstance(params, dict) else {},
                        "description": desc,
                    }
                )
    return schemas


def _parse_seed_tool_calls(text: str) -> List[Dict[str, Any]]:
    """Parse <seed:tool_call> blocks into a list of {function, parameters} dicts.
    Also captures optional <gui_think>...</gui_think> as reasoning.
    """
    actions: List[Dict[str, Any]] = []
    if not text:
        return actions

    # Extract reasoning if present
    reasoning_text = None
    think_match = re.search(r"<gui_think>([\s\S]*?)</gui_think>", text)
    if think_match:
        reasoning_text = think_match.group(1).strip()

    # Iterate each seed tool_call block
    for block in re.finditer(r"<seed:tool_call>([\s\S]*?)</seed:tool_call>", text):
        content = block.group(1)
        # One or multiple <function=...>...</function> inside
        for fmatch in re.finditer(r"<function=([\w_]+)>([\s\S]*?)</function>", content):
            fname = fmatch.group(1)
            inner = fmatch.group(2)
            params: Dict[str, str] = {}
            for pmatch in re.finditer(r"<parameter=([\w_]+)>([\s\S]*?)</parameter>", inner):
                pname = pmatch.group(1)
                pval = pmatch.group(2).strip()
                params[pname] = pval
            actions.append({"function": fname, "parameters": params})

    # If we have a global reasoning and at least one action, attach it to first
    if reasoning_text and actions:
        actions[0]["reasoning"] = reasoning_text
    elif reasoning_text:
        actions.append({"function": "reasoning", "parameters": {"content": reasoning_text}})

    return actions


def _normalize_xy_to_uitars(x: int, y: int, width: int, height: int) -> Tuple[int, int]:
    width = max(1, int(width))
    height = max(1, int(height))
    nx = max(0, min(1000, int(round((x / width) * 1000))))
    ny = max(0, min(1000, int(round((y / height) * 1000))))
    return nx, ny


def _denormalize_xy_from_uitars(nx: float, ny: float, width: int, height: int) -> Tuple[int, int]:
    width = max(1, int(width))
    height = max(1, int(height))
    x = int(round((nx / 1000.0) * width))
    y = int(round((ny / 1000.0) * height))
    return x, y


def _map_computer_action_to_function(
    action: Dict[str, Any], width: int, height: int
) -> Optional[Dict[str, Any]]:
    """Map a computer action item to a UITARS function + parameters dict of strings.
    Returns dict like {"function": name, "parameters": {..}} or None if unknown.
    """
    atype = action.get("type") or action.get("action")
    if atype == "click":
        x, y = action.get("x"), action.get("y")
        btn = action.get("button", "left")
        if x is None or y is None:
            return None
        nx, ny = _normalize_xy_to_uitars(int(x), int(y), width, height)
        if btn == "right":
            return {
                "function": "right_single",
                "parameters": {"point": f"<point>{nx} {ny}</point>"},
            }
        return {"function": "click", "parameters": {"point": f"<point>{nx} {ny}</point>"}}
    if atype == "double_click":
        x, y = action.get("x"), action.get("y")
        if x is None or y is None:
            return None
        nx, ny = _normalize_xy_to_uitars(int(x), int(y), width, height)
        return {"function": "left_double", "parameters": {"point": f"<point>{nx} {ny}</point>"}}
    if atype == "move":
        x, y = action.get("x"), action.get("y")
        if x is None or y is None:
            return None
        nx, ny = _normalize_xy_to_uitars(int(x), int(y), width, height)
        return {"function": "move_to", "parameters": {"point": f"<point>{nx} {ny}</point>"}}
    if atype == "keypress":
        keys = action.get("keys", [])
        if isinstance(keys, list) and keys:
            if len(keys) == 1:
                return {"function": "press", "parameters": {"key": keys[0]}}
            else:
                return {"function": "hotkey", "parameters": {"key": " ".join(keys)}}
        return None
    if atype == "type":
        text = action.get("text", "")
        return {"function": "type", "parameters": {"content": text}}
    if atype == "scroll":
        x, y = action.get("x", 512), action.get("y", 512)
        nx, ny = _normalize_xy_to_uitars(int(x), int(y), width, height)
        sx, sy = action.get("scroll_x", 0), action.get("scroll_y", 0)
        # Our parser used positive sy for up
        direction = (
            "up"
            if sy and sy > 0
            else (
                "down"
                if sy and sy < 0
                else ("right" if sx and sx > 0 else ("left" if sx and sx < 0 else "down"))
            )
        )
        return {
            "function": "scroll",
            "parameters": {"direction": direction, "point": f"<point>{nx} {ny}</point>"},
        }
    if atype == "drag":
        path = action.get("path", [])
        if isinstance(path, list) and len(path) >= 2:
            sx, sy = path[0].get("x"), path[0].get("y")
            ex, ey = path[-1].get("x"), path[-1].get("y")
            if sx is None or sy is None or ex is None or ey is None:
                return None
            nsx, nsy = _normalize_xy_to_uitars(int(sx), int(sy), width, height)
            nex, ney = _normalize_xy_to_uitars(int(ex), int(ey), width, height)
            return {
                "function": "drag",
                "parameters": {
                    "start_point": f"<point>{nsx} {nsy}</point>",
                    "end_point": f"<point>{nex} {ney}</point>",
                },
            }
        return None
    if atype == "wait":
        return {"function": "wait", "parameters": {}}
    if atype == "screenshot":
        return {"function": "take_screenshot", "parameters": {}}
    # Fallback unknown
    return None


def _to_uitars_messages(
    messages: List[Dict[str, Any]], width: int, height: int
) -> List[Dict[str, Any]]:
    """Convert responses items into completion messages tailored for UI-TARS.

    - User content is passed through similar to convert_responses_items_to_completion_messages
    - Assistant/tool history is rendered as text with <gui_think> and <seed:tool_call> blocks
    """
    uitars_messages: List[Dict[str, Any]] = []

    def flush_seed_block(pending_think: Optional[str], pending_functions: List[Dict[str, Any]]):
        if not pending_think and not pending_functions:
            return
        parts: List[str] = []
        if pending_think:
            parts.append(f"<gui_think> {pending_think} </gui_think>")
        if pending_functions:
            inner = []
            for f in pending_functions:
                fname = f["function"]
                params = f.get("parameters", {})
                param_blocks = []
                for k, v in params.items():
                    param_blocks.append(f"<parameter={k}>{v}</parameter>")
                inner.append(f"<function={fname}>{''.join(param_blocks)}</function>")
            parts.append(f"<seed:tool_call>{''.join(inner)}</seed:tool_call>")
        uitars_messages.append({"role": "assistant", "content": "".join(parts)})

    # Accumulators for a single assistant seed block
    pending_think: Optional[str] = None
    pending_functions: List[Dict[str, Any]] = []

    for msg in messages:
        mtype = msg.get("type")
        role = msg.get("role")

        # On any user message, flush current assistant block
        if role == "user" or mtype == "user":
            flush_seed_block(pending_think, pending_functions)
            pending_think, pending_functions = None, []

            content = msg.get("content", "")
            if isinstance(content, list):
                completion_content = []
                for item in content:
                    if item.get("type") == "input_image":
                        completion_content.append(
                            {"type": "image_url", "image_url": {"url": item.get("image_url")}}
                        )
                    elif item.get("type") in ("input_text", "text"):
                        completion_content.append({"type": "text", "text": item.get("text")})
                uitars_messages.append({"role": "user", "content": completion_content})
            elif isinstance(content, str):
                uitars_messages.append({"role": "user", "content": content})
            continue

        # Reasoning item
        if mtype == "reasoning":
            # Responses reasoning stores summary list
            summary = msg.get("summary", [])
            texts = [
                s.get("text", "")
                for s in summary
                if isinstance(s, dict) and s.get("type") == "summary_text"
            ]
            if texts:
                pending_think = "\n".join([t for t in texts if t])
            continue

        # Computer/tool calls -> map to functions
        if mtype == "computer_call":
            f = _map_computer_action_to_function(msg.get("action", {}), width, height)
            if f:
                pending_functions.append(f)
            continue
        if mtype == "function_call":
            # Include custom tools as-is
            name = msg.get("name")
            try:
                args_obj = json.loads(msg.get("arguments", "{}"))
            except json.JSONDecodeError:
                args_obj = {}
            # Ensure string values
            params = {k: (str(v) if not isinstance(v, str) else v) for k, v in args_obj.items()}
            pending_functions.append({"function": name, "parameters": params})
            continue

        # If assistant message text is given, flush current block and add as plain assistant text
        if role == "assistant" or mtype == "message":
            flush_seed_block(pending_think, pending_functions)
            pending_think, pending_functions = None, []
            content = msg.get("content", [])
            if isinstance(content, list):
                texts = [
                    c.get("text", "")
                    for c in content
                    if isinstance(c, dict) and c.get("type") in ("output_text", "text")
                ]
                if texts:
                    uitars_messages.append(
                        {"role": "assistant", "content": "\n".join([t for t in texts if t])}
                    )
            elif isinstance(content, str) and content:
                uitars_messages.append({"role": "assistant", "content": content})
            continue

        # On outputs, flush pending assistant block and send outputs as user messages
        if mtype in ("function_call_output", "computer_call_output"):
            flush_seed_block(pending_think, pending_functions)
            pending_think, pending_functions = None, []
            output = msg.get("output")
            if isinstance(output, dict) and output.get("type") == "input_image":
                img_url = output.get("image_url")
                if img_url:
                    uitars_messages.append(
                        {
                            "role": "user",
                            "content": [
                                {"type": "image_url", "image_url": {"url": img_url}},
                            ],
                        }
                    )
            elif isinstance(output, str):
                uitars_messages.append({"role": "user", "content": output})
            else:
                # Fallback stringify
                uitars_messages.append({"role": "user", "content": json.dumps(output)})
            continue

    # Flush any remaining pending seed block
    flush_seed_block(pending_think, pending_functions)

    return uitars_messages


def _to_response_items(
    actions: List[Dict[str, Any]],
    tool_names: Optional[set[str]] = None,
    width: Optional[int] = None,
    height: Optional[int] = None,
) -> List[Any]:
    """Map parsed actions into Responses items (computer actions + optional reasoning)."""
    items: List[Any] = []
    tool_names = tool_names or set()

    # Optional top-level reasoning attached to first
    if actions and actions[0].get("reasoning"):
        items.append(make_reasoning_item(actions[0]["reasoning"]))

    # Dimensions default
    w = int(width) if width else 1024
    h = int(height) if height else 768

    for a in actions:
        fn = a.get("function")
        params = a.get("parameters", {})
        if fn == "reasoning":
            items.append(make_reasoning_item(params.get("content", "")))
        elif fn in ("click", "left_double", "right_single"):
            # params.point is like: <point>x y</point> or plain "x y"
            point = params.get("point", "").strip()
            m = re.search(r"([\-\d\.]+)\s+([\-\d\.]+)", point)
            if not m:
                continue
            nx = float(m.group(1))
            ny = float(m.group(2))
            x, y = _denormalize_xy_from_uitars(nx, ny, w, h)
            if fn == "left_double":
                items.append(make_double_click_item(x, y))
            elif fn == "right_single":
                items.append(make_click_item(x, y, "right"))
            else:
                items.append(make_click_item(x, y, "left"))
        elif fn == "move_to":
            point = params.get("point", "").strip()
            m = re.search(r"([\-\d\.]+)\s+([\-\d\.]+)", point)
            if not m:
                continue
            nx = float(m.group(1))
            ny = float(m.group(2))
            x, y = _denormalize_xy_from_uitars(nx, ny, w, h)
            items.append(make_move_item(x, y))
        elif fn == "drag":
            sp = params.get("start_point", "").strip()
            ep = params.get("end_point", "").strip()
            ms = re.search(r"([\-\d\.]+)\s+([\-\d\.]+)", sp)
            me = re.search(r"([\-\d\.]+)\s+([\-\d\.]+)", ep)
            if not (ms and me):
                continue
            nsx, nsy = float(ms.group(1)), float(ms.group(2))
            nex, ney = float(me.group(1)), float(me.group(2))
            sx, sy = _denormalize_xy_from_uitars(nsx, nsy, w, h)
            ex, ey = _denormalize_xy_from_uitars(nex, ney, w, h)
            items.append(make_drag_item([{"x": sx, "y": sy}, {"x": ex, "y": ey}]))
        elif fn == "hotkey":
            key = params.get("key", "")
            keys = key.split()
            if keys:
                items.append(make_keypress_item(keys))
        elif fn == "press":
            key = params.get("key", "")
            if key:
                items.append(make_keypress_item([key]))
        elif fn == "type":
            content = params.get("content", "")
            items.append(make_type_item(content))
        elif fn == "scroll":
            # direction: up/down/left/right. Point optional
            direction = params.get("direction", "down").lower()
            point = params.get("point", "")
            m = re.search(r"([\-\d\.]+)\s+([\-\d\.]+)", point)
            if m:
                nx = float(m.group(1))
                ny = float(m.group(2))
                x, y = _denormalize_xy_from_uitars(nx, ny, w, h)
            else:
                x, y = _denormalize_xy_from_uitars(500.0, 500.0, w, h)
            dy = 5 if direction == "up" else -5
            dx = 5 if direction == "right" else (-5 if direction == "left" else 0)
            items.append(make_scroll_item(x, y, dx, dy))
        elif fn == "wait":
            items.append(make_wait_item())
        elif fn == "finished":
            content = params.get("content", "")
            items.append(make_output_text_item(content or "Task completed."))
            break
        elif fn == "take_screenshot":
            items.append(make_screenshot_item())
        elif fn == "open_computer":
            items.append(make_screenshot_item())
        else:
            # If this function name is present in provided tool schemas, emit function_call
            if fn in tool_names:
                # Convert simple string params into an arguments object
                # Parameters are strings; pass through as-is
                items.append(make_function_call_item(fn, params))
            else:
                # Unknown function -> surface as assistant text
                items.append(make_output_text_item(f"Unknown action: {fn} {params}"))

    return items


@register_agent(models=r"(?i).*ui-?tars-?2.*")
class UITARS2Config:
    async def predict_step(
        self,
        messages: List[Dict[str, Any]],
        model: str,
        tools: Optional[List[Dict[str, Any]]] = None,
        max_retries: Optional[int] = None,
        stream: bool = False,
        computer_handler=None,
        use_prompt_caching: Optional[bool] = False,
        _on_api_start=None,
        _on_api_end=None,
        _on_usage=None,
        _on_screenshot=None,
        **kwargs,
    ) -> Dict[str, Any]:
        # Determine screen dimensions (prefer computer_handler, fallback to last screenshot)
        width: Optional[int] = None
        height: Optional[int] = None
        if computer_handler is not None and hasattr(computer_handler, "get_dimensions"):
            try:
                dims = await computer_handler.get_dimensions()  # type: ignore
                if isinstance(dims, (list, tuple)) and len(dims) == 2:
                    width, height = int(dims[0]), int(dims[1])
            except Exception:
                pass

        if width is None or height is None:
            try:
                last_out = get_last_computer_call_output(messages)  # type: ignore
                if last_out:
                    image_url = last_out.get("output", {}).get("image_url", "")
                    if image_url:
                        b64 = image_url.split(",")[-1]
                        img_bytes = base64.b64decode(b64)
                        if Image is not None:
                            img = Image.open(io.BytesIO(img_bytes))
                            width, height = img.size
            except Exception:
                pass

        if width is None or height is None:
            width, height = 1024, 768

        # Convert Responses items to UI-TARS style messages with <seed:tool_call> history
        completion_messages = _to_uitars_messages(messages, width, height)

        # Build dynamic system prompt by concatenating built-in schemas and provided function tools
        provided_fn_schemas = _extract_function_schemas_from_tools(tools)
        combined_schemas = (
            TOOL_SCHEMAS + provided_fn_schemas if provided_fn_schemas else TOOL_SCHEMAS
        )
        dynamic_system_prompt = (
            _PROMPT_PREFIX + _format_tool_schemas_json_lines(combined_schemas) + _PROMPT_SUFFIX
        )

        # Prepend system prompt (based on training prompts + provided tools)
        litellm_messages: List[Dict[str, Any]] = [
            {"role": "system", "content": dynamic_system_prompt},
        ]
        litellm_messages.extend(completion_messages)

        api_kwargs: Dict[str, Any] = {
            "model": model,
            "messages": litellm_messages,
            "max_retries": max_retries,
            "stream": stream,
            **{k: v for k, v in kwargs.items()},
        }
        if use_prompt_caching:
            api_kwargs["use_prompt_caching"] = use_prompt_caching

        if _on_api_start:
            await _on_api_start(api_kwargs)

        response = await litellm.acompletion(**api_kwargs)

        if _on_api_end:
            await _on_api_end(api_kwargs, response)

        usage = {
            **LiteLLMCompletionResponsesConfig._transform_chat_completion_usage_to_responses_usage(  # type: ignore
                response.usage
            ).model_dump(),
            "response_cost": response._hidden_params.get("response_cost", 0.0),
        }
        if _on_usage:
            await _on_usage(usage)

        # Extract text content (first choice)
        response_dict = response.model_dump()  # type: ignore
        content_text = ""
        choices = response_dict.get("choices", [])
        if choices:
            msg = choices[0].get("message", {})
            # message.content may be string or array; gather text pieces
            mc = msg.get("content")
            if isinstance(mc, str):
                content_text = mc
            elif isinstance(mc, list):
                parts = []
                for part in mc:
                    if isinstance(part, dict) and part.get("type") == "text":
                        parts.append(part.get("text", ""))
                content_text = "\n".join([p for p in parts if p])

        # Parse the seed tool calls and map to response items
        actions = _parse_seed_tool_calls(content_text)
        # Build set of tool names from provided tools to emit function_call items
        tool_names: set[str] = set()
        for s in provided_fn_schemas:
            name = s.get("name")
            if isinstance(name, str):
                tool_names.add(name)
        output_items = _to_response_items(actions, tool_names, width, height)

        return {"output": output_items, "usage": usage}

    def get_capabilities(self) -> List[AgentCapability]:
        return ["step"]

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        """Predict a single click coordinate using a minimal prompt with a click tool.

        This sends the current screenshot and instruction, asking the model to
        output a click action in the form:
            Action: click(point='(x,y)')
        """
        # Minimal grounding-style prompt
        system_text = (
            "You are a GUI agent. Given the instruction, return a single action on the current screen.\n\n"
            "## Output Format\n\n"
            "Action: click(point='(x,y)')\n\n"
            "## User Instruction\n"
            f"{instruction}"
        )

        # Build messages with image
        litellm_messages: List[Dict[str, Any]] = [
            {"role": "system", "content": system_text},
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Please return a single click action."},
                    {
                        "type": "image_url",
                        "image_url": {"url": f"data:image/png;base64,{image_b64}"},
                    },
                ],
            },
        ]

        api_kwargs: Dict[str, Any] = {
            "model": model,
            "messages": litellm_messages,
            "max_tokens": kwargs.get("max_tokens", 512),
            "temperature": kwargs.get("temperature", 0.0),
            "do_sample": kwargs.get("temperature", 0.0) > 0.0,
        }
        api_kwargs.update(
            {k: v for k, v in (kwargs or {}).items() if k not in ["max_tokens", "temperature"]}
        )

        response = await litellm.acompletion(**api_kwargs)
        # Extract response content
        response_dict = response.model_dump()  # type: ignore
        choices = response_dict.get("choices", [])
        if not choices:
            return None
        msg = choices[0].get("message", {})
        content_text = msg.get("content", "")
        if isinstance(content_text, list):
            text_parts = [
                p.get("text", "")
                for p in content_text
                if isinstance(p, dict) and p.get("type") == "text"
            ]
            content_text = "\n".join([t for t in text_parts if t])
        if not isinstance(content_text, str):
            return None

        # Parse coordinates
        # Pattern for click(point='(x,y)') or click(start_box='(x,y)')
        patterns = [
            r"click\(point='\((\d+),(\d+)\)'\)",
            r"click\((?:start_box|point)='\((\d+),(\d+)\)'\)",
        ]
        for pat in patterns:
            m = re.search(pat, content_text)
            if m:
                try:
                    x, y = int(m.group(1)), int(m.group(2))
                    return (x, y)
                except Exception:
                    pass
        return None
