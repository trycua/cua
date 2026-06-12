"""
Prompt instructions callback.

This callback allows simple prompt engineering by pre-pending a user
instructions message to the start of the conversation before each LLM call.

Usage:

    from cua_agent.callbacks import PromptInstructionsCallback
    agent = ComputerAgent(
        model="openai/computer-use-preview",
        callbacks=[PromptInstructionsCallback("Follow these rules...")]
    )

"""

from typing import Any, Dict, List, Optional

from .base import AsyncCallbackHandler


class PromptInstructionsCallback(AsyncCallbackHandler):
    """
    Prepend a user instructions message to the message list.

    This is a minimal, non-invasive way to guide the agent's behavior without
    modifying agent loops or tools. It works with any provider/loop since it
    only alters the messages array before sending to the model.
    """

    def __init__(self, instructions: Optional[str]) -> None:
        self.instructions = instructions

    async def on_llm_start(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        # Pre-pend instructions message
        if not self.instructions:
            return messages

        # Ensure we don't duplicate if already present at the front
        if messages and isinstance(messages[0], dict):
            first = messages[0]
            if first.get("role") == "user" and first.get("content") == self.instructions:
                return messages

        return [
            {"role": "user", "content": self.instructions},
        ] + messages
