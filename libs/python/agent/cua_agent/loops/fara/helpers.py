# Source: https://github.com/QwenLM/Qwen-Agent/blob/main/qwen_agent/llm/fncall_prompts/nous_fncall_prompt.py

import copy
import json
import os
from typing import Any, Dict, List, Literal, Optional, Tuple, Union

from .schema import ContentItem, Message

FN_CALL_TEMPLATE_QWEN = """# Tools

You may call one or more functions to assist with the user query.

You are provided with function signatures within <tools></tools> XML tags:
<tools>
{tool_descs}
</tools>

For each function call, return a json object with function name and arguments within <tool_call></tool_call> XML tags:
<tool_call>
{{"name": <function-name>, "arguments": <args-json-object>}}
</tool_call>"""

FN_CALL_TEMPLATE = """You are a web automation agent that performs actions on websites to fulfill user requests by calling various tools.
* You should stop execution at Critical Points. A Critical Point would be encountered in tasks like 'Checkout', 'Book', 'Purchase', 'Call', 'Email', 'Order', etc where a binding transaction/agreement would require the user's permission/personal or sensitive information (name, email, credit card, address, payment information, resume, etc) in order to complete a transaction (purchase, reservation, sign-up etc), or to communicate in a way that a human would be expected to do (call, email, apply to a job, etc).
* Solve the task as far as you can up until a Critical Point:
    - For example, if the task is to "call a restaurant to make a reservation", you should not actually make the call but should navigate to the restaurant's page and find the phone number.
    - Similarly, if the task is to "order new size 12 running shoes" you should not actually place the order but should instead search for the right shoes that meet the criteria and add them to the cart.
    - Some tasks, like answering questions, may not encounter a Critical Point at all.

You are provided with function signatures within <tools></tools> XML tags:
<tools>
{tool_descs}
</tools>

For each function call, return a json object with function name and arguments within <tool_call></tool_call> XML tags:
<tool_call>
{{"name": <function-name>, "arguments": <args-json-object>}}
</tool_call>"""


SPECIAL_CODE_MODE = os.getenv("SPECIAL_CODE_MODE", "false").lower() == "true"
CODE_TOOL_PATTERN = "code_interpreter"
FN_CALL_TEMPLATE_WITH_CI = """# Tools

You may call one or more functions to assist with the user query.

You are provided with function signatures within <tools></tools> XML tags:
<tools>
{tool_descs}
</tools>

For each function call, return a json object with function name and arguments within <tool_call></tool_call> XML tags:
<tool_call>
{{"name": <function-name>, "arguments": <args-json-object>}}
</tool_call>
For code parameters, use placeholders first, and then put the code within <code></code> XML tags, such as:
<tool_call>
{{"name": <function-name>, "arguments": {{"code": ""}}}}
<code>
Here is the code.
</code>
</tool_call>"""


class NousFnCallPrompt:
    def __init__(self, template_name: str = "default"):
        """Initialize NousFnCallPrompt with a specific template.

        Args:
            template_name: Name of the template to use. Options:
                          "default", "qwen", "with_ci"
        """
        self.template_name = template_name
        self.template_map = {
            "default": FN_CALL_TEMPLATE,
            "qwen": FN_CALL_TEMPLATE_QWEN,
            "with_ci": FN_CALL_TEMPLATE_WITH_CI,
        }

        if template_name not in self.template_map:
            raise ValueError(
                f"Unknown template_name: {template_name}. "
                f"Available options: {list(self.template_map.keys())}"
            )

    def preprocess_fncall_messages(
        self,
        messages: List[Message],
        functions: List[dict],
        lang: Literal["en", "zh"],
        parallel_function_calls: bool = True,
        function_choice: Union[Literal["auto"], str] = "auto",
    ) -> List[Message]:
        del lang  # ignored
        del parallel_function_calls  # ignored
        if function_choice != "auto":
            raise NotImplementedError

        ori_messages = messages

        # Change function_call responses to plaintext responses:
        messages = []
        for msg in copy.deepcopy(ori_messages):
            role, content, reasoning_content = (
                msg.role,
                msg.content,
                msg.reasoning_content,
            )
            if role in ("system", "user"):
                messages.append(msg)
            elif role == "assistant":
                content = content or []
                fn_call = msg.function_call
                if fn_call:
                    if (not SPECIAL_CODE_MODE) or (CODE_TOOL_PATTERN not in fn_call.name):
                        fc = {
                            "name": fn_call.name,
                            "arguments": json.loads(fn_call.arguments),
                        }
                        fc = json.dumps(fc, ensure_ascii=False)
                        fc = f"<tool_call>\n{fc}\n</tool_call>"
                    else:
                        para = json.loads(fn_call.arguments)
                        code = para["code"]
                        para["code"] = ""
                        fc = {"name": fn_call.name, "arguments": para}
                        fc = json.dumps(fc, ensure_ascii=False)
                        fc = f"<tool_call>\n{fc}\n<code>\n{code}\n</code>\n</tool_call>"

                    content.append(ContentItem(text=fc))
                if messages[-1].role == "assistant":
                    messages[-1].content.append(ContentItem(text="\n"))
                    messages[-1].content.extend(content)
                else:
                    # TODO: Assuming there will only be one continuous reasoning_content here
                    messages.append(
                        Message(
                            role=role,
                            content=content,
                            reasoning_content=reasoning_content,
                        )
                    )
            elif role == "function":
                assert isinstance(content, list)
                assert len(content) == 1
                assert content[0].text
                fc = f"<tool_response>\n{content[0].text}\n</tool_response>"
                content = [ContentItem(text=fc)]
                if messages[-1].role == "user":
                    messages[-1].content.append(ContentItem(text="\n"))
                    messages[-1].content.extend(content)
                else:
                    messages.append(Message(role="user", content=content))
            else:
                raise TypeError

        tool_descs = [{"type": "function", "function": f} for f in functions]
        tool_names = [
            function.get("name_for_model", function.get("name", "")) for function in functions
        ]
        tool_descs = "\n".join([json.dumps(f, ensure_ascii=False) for f in tool_descs])

        # Select template based on configuration
        if SPECIAL_CODE_MODE and any([CODE_TOOL_PATTERN in x for x in tool_names]):
            selected_template = FN_CALL_TEMPLATE_WITH_CI
        else:
            selected_template = self.template_map[self.template_name]

        tool_system = selected_template.format(tool_descs=tool_descs)
        if messages[0].role == "system":
            messages[0].content.append(ContentItem(text="\n\n" + tool_system))
        else:
            messages = [Message(role="system", content=[ContentItem(text=tool_system)])] + messages
        return messages


# Mainly for removing incomplete special tokens when streaming the output
# This assumes that '<tool_call>\n{"name": "' is the special token for the NousFnCallPrompt
def remove_incomplete_special_tokens(text: str) -> str:
    if text in '<tool_call>\n{"name": "':
        text = ""
    return text


def extract_fn(text: str):
    fn_name, fn_args = "", ""
    fn_name_s = '"name": "'
    fn_name_e = '", "'
    fn_args_s = '"arguments": '
    i = text.find(fn_name_s)
    k = text.find(fn_args_s)
    if i > 0:
        _text = text[i + len(fn_name_s) :]
        j = _text.find(fn_name_e)
        if j > -1:
            fn_name = _text[:j]
    if k > 0:
        fn_args = text[k + len(fn_args_s) :]

    if len(fn_args) > 5:
        fn_args = fn_args[:-5]
    else:
        fn_args = ""
    return fn_name, fn_args


def build_nous_system(functions: List[Dict[str, Any]]) -> Optional[Dict[str, Any]]:
    """Use original FARA NousFnCallPrompt to generate a system message embedding tool schema."""
    from .schema import ContentItem as NousContentItem
    from .schema import Message as NousMessage

    msgs = NousFnCallPrompt().preprocess_fncall_messages(
        messages=[
            NousMessage(
                role="system", content=[NousContentItem(text="You are a helpful assistant.")]
            )
        ],
        functions=functions,
        lang="en",
    )
    sys = msgs[0].model_dump()
    # Convert structured content to OpenAI-style content list
    content = [{"type": "text", "text": c["text"]} for c in sys.get("content", [])]
    return {"role": "system", "content": content}


def fix_fara_tool_call_format(messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """
    Fix tool call format in conversation history for FARA compatibility.

    The shared `convert_responses_items_to_completion_messages` function outputs:
    - Tool name as "computer" (should be "computer_use")
    - Action key as "type" (should be "action")

    This function post-processes assistant messages to fix these issues.
    """
    import re

    # Valid FARA action types
    valid_actions = {
        "left_click",
        "right_click",
        "middle_click",
        "double_click",
        "triple_click",
        "click",
        "type",
        "key",
        "scroll",
        "hscroll",
        "mouse_move",
        "wait",
        "visit_url",
        "web_search",
        "history_back",
        "screenshot",
        "terminate",
    }

    fixed_messages = []
    for msg in messages:
        if msg.get("role") != "assistant":
            fixed_messages.append(msg)
            continue

        content = msg.get("content", "")
        if not isinstance(content, str) or "<tool_call>" not in content:
            fixed_messages.append(msg)
            continue

        # Find and fix all tool calls in the content
        def fix_tool_call(match):
            tool_call_content = match.group(1)
            try:
                tool_call = json.loads(tool_call_content)

                # Fix tool name: "computer" -> "computer_use"
                if tool_call.get("name") == "computer":
                    tool_call["name"] = "computer_use"

                # Fix arguments: "type" -> "action" and x/y -> coordinate
                args = tool_call.get("arguments", {})
                if isinstance(args, dict):
                    # If "type" contains a valid action, rename to "action"
                    if "type" in args and args["type"] in valid_actions:
                        args["action"] = args.pop("type")

                    # Convert internal x/y format back to FARA coordinate format
                    if "x" in args and "y" in args and "coordinate" not in args:
                        args["coordinate"] = [args.pop("x"), args.pop("y")]

                    # Normalize action names: "click" -> "left_click"
                    if args.get("action") == "click":
                        args["action"] = "left_click"

                    # Remove "button" field - FARA doesn't use it (action name implies button)
                    args.pop("button", None)

                    # If "action" is empty but we can infer from other keys
                    if args.get("action") == "" and "coordinate" in args:
                        args["action"] = "left_click"

                    tool_call["arguments"] = args

                return f"<tool_call>\n{json.dumps(tool_call)}\n</tool_call>"
            except (json.JSONDecodeError, TypeError):
                return match.group(0)  # Return original if parsing fails

        # Match <tool_call>...</tool_call> or <tool_call>...</tool_call>
        fixed_content = re.sub(
            r"<tool_call>\s*(\{.*?\})\s*</tool_call>", fix_tool_call, content, flags=re.DOTALL
        )

        # Also handle malformed closing tags like <tool_call> used as closing
        fixed_content = re.sub(
            r"<tool_call>(\{.*?\})<tool_call>", fix_tool_call, fixed_content, flags=re.DOTALL
        )

        fixed_messages.append({**msg, "content": fixed_content})

    return fixed_messages


def parse_tool_call_from_text(text: str) -> Optional[Dict[str, Any]]:
    """Extract JSON object within <tool_call>...</tool_call> from model text.

    Accepts both </tool_call> and <tool_call> as closing tags for robustness.
    Handles nested braces in JSON objects.
    """
    # Find the opening tag
    start_idx = text.find("<tool_call>")
    if start_idx == -1:
        return None

    # Find the start of JSON (first '{' after opening tag)
    json_start = text.find("{", start_idx)
    if json_start == -1:
        return None

    # Extract JSON by counting braces
    brace_count = 0
    json_end = json_start
    for i in range(json_start, len(text)):
        if text[i] == "{":
            brace_count += 1
        elif text[i] == "}":
            brace_count -= 1
            if brace_count == 0:
                json_end = i + 1
                break

    if brace_count != 0:
        return None

    json_str = text[json_start:json_end]
    try:
        return json.loads(json_str)
    except Exception:
        return None


async def unnormalize_coordinate(args: Dict[str, Any], dims: Tuple[int, int]) -> Dict[str, Any]:
    """Coordinates appear in 0..1000 space, scale to actual screen size using dims if provided."""
    coord = args.get("coordinate")
    if not coord or not isinstance(coord, (list, tuple)) or len(coord) < 2:
        return args
    x, y = float(coord[0]), float(coord[1])
    width, height = float(dims[0]), float(dims[1])
    x_abs = max(0.0, min(width, (x / 1000.0) * width))
    y_abs = max(0.0, min(height, (y / 1000.0) * height))
    args = {**args, "coordinate": [round(x_abs), round(y_abs)]}
    return args


def convert_qwen_tool_args_to_computer_action(args: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    """
    Convert Qwen computer tool arguments to the Computer Calls action schema.

    Qwen (example):
        {"action": "left_click", "coordinate": [114, 68]}

    Target (example):
        {"action": "left_click", "x": 114, "y": 68}

    Other mappings:
    - right_click, middle_click, double_click (triple_click -> double_click)
    - mouse_move -> { action: "move", x, y }
    - key -> { action: "keypress", keys: [...] }
    - type -> { action: "type", text }
    - scroll/hscroll -> { action: "scroll", scroll_x, scroll_y, x, y }
    - wait -> { action: "wait" }
    - terminate/answer are not direct UI actions; return None for now
    """
    if not isinstance(args, dict):
        return None

    action = args.get("action")
    if not isinstance(action, str):
        return None

    # Coordinates helper
    coord = args.get("coordinate")
    x = y = None
    if isinstance(coord, (list, tuple)) and len(coord) >= 2:
        try:
            x = int(round(float(coord[0])))
            y = int(round(float(coord[1])))
        except Exception:
            x = y = None

    # Map actions
    a = action.lower()
    if a in {"left_click", "right_click", "middle_click", "double_click"}:
        if x is None or y is None:
            return None
        return {"action": a, "x": x, "y": y}
    if a == "triple_click":
        # Approximate as double_click
        if x is None or y is None:
            return None
        return {"action": "double_click", "x": x, "y": y}
    if a == "mouse_move":
        if x is None or y is None:
            return None
        return {"action": "move", "x": x, "y": y}
    if a == "key":
        keys = args.get("keys")
        if isinstance(keys, list) and all(isinstance(k, str) for k in keys):
            return {"action": "keypress", "keys": keys}
        return None
    if a == "type":
        text = args.get("text")
        if isinstance(text, str):
            return {"action": "type", "text": text}
        return None
    if a in {"scroll", "hscroll"}:
        pixels = args.get("pixels") or 0
        try:
            pixels_val = int(round(float(pixels)))
        except Exception:
            pixels_val = 0
        scroll_x = pixels_val if a == "hscroll" else 0
        scroll_y = pixels_val if a == "scroll" else 0
        # Include cursor position if available (optional)
        out: Dict[str, Any] = {"action": "scroll", "scroll_x": scroll_x, "scroll_y": scroll_y}
        if x is not None and y is not None:
            out.update({"x": x, "y": y})
        return out
    if a == "wait":
        return {"action": "wait"}

    # Non-UI or terminal actions: terminate/answer -> not mapped here
    return None


def convert_fara_args_to_browser_tool_format(args: Dict[str, Any]) -> Dict[str, Any]:
    """
    Convert FARA model output format to BrowserTool compatible format.

    FARA model may output extra parameters that BrowserTool methods don't accept.
    This function cleans up the arguments and maps them to the correct format.

    Examples:
        Input:  {"action": "click", "button": "left", "x": 378, "y": 144}
        Output: {"action": "left_click", "coordinate": [378, 144]}

        Input:  {"action": "visit_url", "url": "https://...", "text": "..."}
        Output: {"action": "visit_url", "url": "https://..."}

        Input:  {"action": "terminate", "url": "...", "text": "...", "status": "success"}
        Output: {"action": "terminate", "status": "success"}
    """
    if not isinstance(args, dict):
        return args

    action = args.get("action", "")
    if not isinstance(action, str):
        return args

    a = action.lower()
    result: Dict[str, Any] = {"action": a}

    # Handle coordinate-based actions
    # Check for both coordinate array and separate x/y fields
    coord = args.get("coordinate")
    x = args.get("x")
    y = args.get("y")

    if coord and isinstance(coord, (list, tuple)) and len(coord) >= 2:
        x, y = coord[0], coord[1]

    # Click actions - normalize to left_click with coordinate
    if a in {"click", "left_click"}:
        if x is not None and y is not None:
            result["action"] = "left_click"
            result["coordinate"] = [x, y]
        return result

    if a in {"right_click", "middle_click", "double_click", "triple_click"}:
        if x is not None and y is not None:
            result["coordinate"] = [x, y]
        return result

    if a == "mouse_move":
        if x is not None and y is not None:
            result["coordinate"] = [x, y]
        return result

    if a == "left_click_drag":
        if x is not None and y is not None:
            result["coordinate"] = [x, y]
        # Also handle start/end coordinates if present
        start_coord = args.get("start_coordinate")
        end_coord = args.get("end_coordinate")
        if start_coord:
            result["start_coordinate"] = start_coord
        if end_coord:
            result["end_coordinate"] = end_coord
        return result

    # Keyboard actions
    if a == "key":
        keys = args.get("keys")
        if keys:
            result["keys"] = keys
        return result

    if a == "type":
        text = args.get("text")
        if text:
            result["text"] = text
        # Include coordinate if typing at a specific location
        if x is not None and y is not None:
            result["coordinate"] = [x, y]
        return result

    # Scroll actions
    if a in {"scroll", "hscroll"}:
        pixels = args.get("pixels")
        if pixels is not None:
            result["pixels"] = pixels
        if x is not None and y is not None:
            result["coordinate"] = [x, y]
        return result

    # Browser-specific actions
    if a == "visit_url":
        url = args.get("url")
        if url:
            result["url"] = url
        return result

    if a == "web_search":
        query = args.get("query")
        if query:
            result["query"] = query
        return result

    if a == "history_back":
        return result

    # Wait action
    if a == "wait":
        time_val = args.get("time")
        if time_val is not None:
            result["time"] = time_val
        return result

    # Screenshot action
    if a == "screenshot":
        return result

    # Terminate action
    if a == "terminate":
        status = args.get("status", "success")
        result["status"] = status
        return result

    # For any other action, return cleaned args (just action + known fields)
    return result


def _convert_responses_items_to_fara_messages(
    messages: List[Dict[str, Any]],
    allow_images_in_tool_results: bool = False,
) -> List[Dict[str, Any]]:
    """
    Convert SDK responses_items format to FARA-compatible completion messages.

    This is FARA's dedicated conversion layer (similar to Anthropic's pattern).
    It handles the conversion from SDK's OpenAI-style format to FARA's native format:

    SDK format:
        {"type": "click", "x": 100, "y": 200, "button": "left"}

    FARA format (in XML tool_call):
        {"name": "computer_use", "arguments": {"action": "left_click", "coordinate": [100, 200]}}
    """
    completion_messages: List[Dict[str, Any]] = []

    for message in messages:
        msg_type = message.get("type")
        role = message.get("role")

        # Handle user messages
        if role == "user" or msg_type == "user":
            content = message.get("content", "")
            if isinstance(content, list):
                converted_content = []
                for item in content:
                    if isinstance(item, dict):
                        item_type = item.get("type")
                        if item_type == "input_image":
                            image_url = item.get("image_url", "")
                            if image_url and image_url != "[omitted]":
                                converted_content.append(
                                    {"type": "image_url", "image_url": {"url": image_url}}
                                )
                        elif item_type == "input_text":
                            converted_content.append({"type": "text", "text": item.get("text", "")})
                        elif item_type == "image_url":
                            # Already in correct format
                            converted_content.append(item)
                        elif item_type == "text":
                            converted_content.append(item)
                        else:
                            converted_content.append(item)
                    else:
                        converted_content.append({"type": "text", "text": str(item)})
                completion_messages.append({"role": "user", "content": converted_content})
            else:
                completion_messages.append({"role": "user", "content": content})

        # Handle assistant messages
        elif role == "assistant" and msg_type == "message":
            content = message.get("content", [])
            if isinstance(content, str):
                completion_messages.append({"role": "assistant", "content": content})
            elif isinstance(content, list):
                text_parts = []
                for item in content:
                    if isinstance(item, dict) and item.get("type") == "output_text":
                        text_parts.append(item.get("text", ""))
                completion_messages.append({"role": "assistant", "content": "\n".join(text_parts)})

        # Handle reasoning
        elif msg_type == "reasoning":
            summary = message.get("summary", [])
            reasoning_text = ""
            if isinstance(summary, list) and summary:
                for item in summary:
                    if isinstance(item, dict) and item.get("type") == "summary_text":
                        reasoning_text = item.get("text", "")
                        break
            if reasoning_text:
                completion_messages.append({"role": "assistant", "content": reasoning_text})

        # Handle computer_call - convert SDK format to FARA's XML tool_call format
        elif msg_type == "computer_call":
            action = message.get("action", {})
            action_type = action.get("type")

            # Convert SDK action to FARA format
            fara_args = _sdk_action_to_fara_args(action)

            # Build FARA's XML tool_call format
            tool_call_json = json.dumps({"name": "computer_use", "arguments": fara_args})
            tool_call_text = f"<tool_call>\n{tool_call_json}\n</tool_call>"

            # Append to last assistant message or create new one
            if completion_messages and completion_messages[-1].get("role") == "assistant":
                prev_content = completion_messages[-1].get("content", "")
                completion_messages[-1]["content"] = f"{prev_content}\n{tool_call_text}".strip()
            else:
                completion_messages.append({"role": "assistant", "content": tool_call_text})

        # Handle computer_call_output - convert to FARA's tool_response format
        elif msg_type == "computer_call_output":
            output = message.get("output", {})

            # Build response content
            if isinstance(output, dict) and output.get("type") == "input_image":
                image_url = output.get("image_url", "")
                response_text = "<tool_response>\nAction executed successfully. Here is the next screenshot.\n</tool_response>"

                # Add as user message with image
                if allow_images_in_tool_results and image_url and image_url != "[omitted]":
                    completion_messages.append(
                        {
                            "role": "user",
                            "content": [
                                {"type": "text", "text": response_text},
                                {"type": "image_url", "image_url": {"url": image_url}},
                            ],
                        }
                    )
                else:
                    completion_messages.append(
                        {
                            "role": "user",
                            "content": [
                                {"type": "text", "text": response_text},
                            ],
                        }
                    )
            elif isinstance(output, dict) and output.get("terminated"):
                response_text = "<tool_response>\nTask terminated.\n</tool_response>"
                completion_messages.append({"role": "user", "content": response_text})
            else:
                response_text = f"<tool_response>\n{json.dumps(output) if isinstance(output, dict) else str(output)}\n</tool_response>"
                completion_messages.append({"role": "user", "content": response_text})

        # Handle function_call (non-computer tools)
        elif msg_type == "function_call":
            fn_name = message.get("name", "")
            fn_args = message.get("arguments", "{}")

            tool_call_json = json.dumps(
                {
                    "name": fn_name,
                    "arguments": json.loads(fn_args) if isinstance(fn_args, str) else fn_args,
                }
            )
            tool_call_text = f"<tool_call>\n{tool_call_json}\n</tool_call>"

            if completion_messages and completion_messages[-1].get("role") == "assistant":
                prev_content = completion_messages[-1].get("content", "")
                completion_messages[-1]["content"] = f"{prev_content}\n{tool_call_text}".strip()
            else:
                completion_messages.append({"role": "assistant", "content": tool_call_text})

        # Handle function_call_output
        elif msg_type == "function_call_output":
            output = message.get("output", "")
            response_text = f"<tool_response>\n{output}\n</tool_response>"
            completion_messages.append({"role": "user", "content": response_text})

    return completion_messages


def _sdk_action_to_fara_args(action: Dict[str, Any]) -> Dict[str, Any]:
    """
    Convert SDK action format to FARA arguments format.

    SDK format:  {"type": "click", "x": 100, "y": 200, "button": "left"}
    FARA format: {"action": "left_click", "coordinate": [100, 200]}
    """
    action_type = action.get("type", "")

    # Click actions
    if action_type == "click":
        button = action.get("button", "left")
        action_name = {
            "left": "left_click",
            "right": "right_click",
            "wheel": "middle_click",
            "middle": "middle_click",
        }.get(button, "left_click")
        return {"action": action_name, "coordinate": [action.get("x", 0), action.get("y", 0)]}

    if action_type == "double_click":
        return {"action": "double_click", "coordinate": [action.get("x", 0), action.get("y", 0)]}

    # Type action
    if action_type == "type":
        result = {"action": "type", "text": action.get("text", "")}
        # Include coordinate if present (for click-then-type)
        if "x" in action and "y" in action:
            result["coordinate"] = [action.get("x", 0), action.get("y", 0)]
        return result

    # Keypress action
    if action_type == "keypress":
        keys = action.get("keys", [])
        return {"action": "key", "keys": keys}

    # Move action
    if action_type in ("move", "mouse_move"):
        return {"action": "mouse_move", "coordinate": [action.get("x", 0), action.get("y", 0)]}

    # Scroll action
    if action_type == "scroll":
        scroll_x = action.get("scroll_x", 0)
        scroll_y = action.get("scroll_y", 0)
        # FARA uses pixels (positive = up/left, negative = down/right)
        pixels = scroll_y if scroll_y != 0 else scroll_x
        result = {"action": "scroll", "pixels": pixels}
        if "x" in action and "y" in action:
            result["coordinate"] = [action.get("x", 0), action.get("y", 0)]
        return result

    # Drag action
    if action_type == "drag":
        path = action.get("path", [])
        if len(path) >= 2:
            return {
                "action": "left_click_drag",
                "start_coordinate": [path[0].get("x", 0), path[0].get("y", 0)],
                "end_coordinate": [path[-1].get("x", 0), path[-1].get("y", 0)],
            }
        return {"action": "left_click_drag"}

    # Screenshot
    if action_type == "screenshot":
        return {"action": "screenshot"}

    # Wait
    if action_type == "wait":
        return {"action": "wait"}

    # Terminate
    if action_type == "terminate":
        return {"action": "terminate", "status": action.get("status", "success")}

    # Fallback - return as-is with type renamed to action
    return {"action": action_type, **{k: v for k, v in action.items() if k != "type"}}
