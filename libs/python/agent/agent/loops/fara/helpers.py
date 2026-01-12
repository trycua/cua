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
