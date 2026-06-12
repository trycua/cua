"""
OperatorValidatorCallback

Ensures agent output actions conform to expected schemas by fixing common issues:
- click: add default button='left' if missing
- keypress: wrap keys string into a list
- etc.

This runs in on_llm_end, which receives the output array (AgentMessage[] as dicts).
The purpose is to avoid spending another LLM call to fix broken computer call syntax when possible.
"""

from __future__ import annotations

from typing import Any, Dict, List

from .base import AsyncCallbackHandler


class OperatorNormalizerCallback(AsyncCallbackHandler):
    """Normalizes common computer call hallucinations / errors in computer call syntax."""

    async def on_llm_end(self, output: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        # Mutate in-place as requested, but still return the list for chaining
        for item in output or []:
            if item.get("type") != "computer_call":
                continue
            action = item.get("action")
            if not isinstance(action, dict):
                continue

            # rename mouse click actions to "click"
            for mouse_btn in ["left", "right", "wheel", "back", "forward"]:
                if action.get("type", "") == f"{mouse_btn}_click":
                    action["type"] = "click"
                    action["button"] = mouse_btn
            # rename hotkey actions to "keypress"
            for alias in ["hotkey", "key", "press", "key_press"]:
                if action.get("type", "") == alias:
                    action["type"] = "keypress"
            # assume click actions
            if "button" in action and "type" not in action:
                action["type"] = "click"
            if "click" in action and "type" not in action:
                action["type"] = "click"
            if ("scroll_x" in action or "scroll_y" in action) and "type" not in action:
                action["type"] = "scroll"
            if "text" in action and "type" not in action:
                action["type"] = "type"

            action_type = action.get("type")

            def _keep_keys(action: Dict[str, Any], keys_to_keep: List[str]):
                """Keep only the provided keys on action; delete everything else.
                Always ensures required 'type' is present if listed in keys_to_keep.
                """
                for key in list(action.keys()):
                    if key not in keys_to_keep:
                        del action[key]

            # rename "coordinate" to "x", "y"
            if "coordinate" in action:
                action["x"] = action["coordinate"][0]
                action["y"] = action["coordinate"][1]
                del action["coordinate"]
            if action_type == "click":
                # convert "click" to "button"
                if "button" not in action and "click" in action:
                    action["button"] = action["click"]
                    del action["click"]
                # default button to "left"
                action["button"] = action.get("button", "left")
            # add default scroll x, y if missing
            if action_type == "scroll":
                action["scroll_x"] = action.get("scroll_x", 0)
                action["scroll_y"] = action.get("scroll_y", 0)
            # ensure keys arg is a list (normalize aliases first)
            if action_type == "keypress":
                keys = action.get("keys")
                for keys_alias in ["keypress", "key", "press", "key_press", "text"]:
                    if keys_alias in action:
                        action["keys"] = action[keys_alias]
                        del action[keys_alias]
                keys = action.get("keys")
                if isinstance(keys, str):
                    action["keys"] = keys.replace("-", "+").split("+") if len(keys) > 1 else [keys]
            required_keys_by_type = {
                # OpenAI actions
                "click": ["type", "button", "x", "y"],
                "double_click": ["type", "x", "y"],
                "drag": ["type", "path"],
                "keypress": ["type", "keys"],
                "move": ["type", "x", "y"],
                "screenshot": ["type"],
                "scroll": ["type", "scroll_x", "scroll_y", "x", "y"],
                "type": ["type", "text"],
                "wait": ["type"],
                # Anthropic actions
                "left_mouse_down": ["type", "x", "y"],
                "left_mouse_up": ["type", "x", "y"],
                "triple_click": ["type", "button", "x", "y"],
            }
            keep = required_keys_by_type.get(action_type or "")
            if keep:
                _keep_keys(action, keep)

        # # Second pass: if an assistant message is immediately followed by a computer_call,
        # # replace the assistant message itself with a reasoning message with summary text.
        # if isinstance(output, list):
        #     for i, item in enumerate(output):
        #         # AssistantMessage shape: { type: 'message', role: 'assistant', content: OutputContent[] }
        #         if item.get("type") == "message" and item.get("role") == "assistant":
        #             next_idx = i + 1
        #             if next_idx >= len(output):
        #                 continue
        #             next_item = output[next_idx]
        #             if not isinstance(next_item, dict):
        #                 continue
        #             if next_item.get("type") != "computer_call":
        #                 continue
        #             contents = item.get("content") or []
        #             # Extract text from OutputContent[]
        #             text_parts: List[str] = []
        #             if isinstance(contents, list):
        #                 for c in contents:
        #                     if isinstance(c, dict) and c.get("type") == "output_text" and isinstance(c.get("text"), str):
        #                         text_parts.append(c["text"])
        #             text_content = "\n".join(text_parts).strip()
        #             # Replace assistant message with reasoning message
        #             output[i] = {
        #                 "type": "reasoning",
        #                 "summary": [
        #                     {
        #                         "type": "summary_text",
        #                         "text": text_content,
        #                     }
        #                 ],
        #             }

        return output
