"""
Image retention callback handler that limits the number of recent images in message history.
"""

from typing import Any, Dict, List, Optional

from .base import AsyncCallbackHandler


class ImageRetentionCallback(AsyncCallbackHandler):
    """
    Callback handler that applies image retention policy to limit the number
    of recent images in message history to prevent context window overflow.
    """

    def __init__(self, only_n_most_recent_images: Optional[int] = None):
        """
        Initialize the image retention callback.

        Args:
            only_n_most_recent_images: If set, only keep the N most recent images in message history
        """
        self.only_n_most_recent_images = only_n_most_recent_images

    async def on_llm_start(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """
        Apply image retention policy to messages before sending to agent loop.

        Args:
            messages: List of message dictionaries

        Returns:
            List of messages with image retention policy applied
        """
        if self.only_n_most_recent_images is None:
            return messages

        return self._apply_image_retention(messages)

    def _apply_image_retention(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Apply image retention policy to keep only the N most recent images.

        Removes computer_call_output items with image_url and their corresponding computer_call items,
        keeping only the most recent N image pairs based on only_n_most_recent_images setting.

        Args:
            messages: List of message dictionaries

        Returns:
            Filtered list of messages with image retention applied
        """
        if self.only_n_most_recent_images is None:
            return messages

        # Gather indices of all computer_call_output messages that contain an image_url
        output_indices: List[int] = []
        for idx, msg in enumerate(messages):
            if msg.get("type") == "computer_call_output":
                out = msg.get("output")
                if isinstance(out, dict) and ("image_url" in out):
                    output_indices.append(idx)

        # Nothing to trim
        if len(output_indices) <= self.only_n_most_recent_images:
            return messages

        # Determine which outputs to keep (most recent N)
        keep_output_indices = set(output_indices[-self.only_n_most_recent_images :])

        # Build set of indices to remove in one pass
        to_remove: set[int] = set()

        for idx in output_indices:
            if idx in keep_output_indices:
                continue  # keep this screenshot and its context

            to_remove.add(idx)  # remove the computer_call_output itself

            # Remove the immediately preceding computer_call with matching call_id (if present)
            call_id = messages[idx].get("call_id")
            prev_idx = idx - 1
            if (
                prev_idx >= 0
                and messages[prev_idx].get("type") == "computer_call"
                and messages[prev_idx].get("call_id") == call_id
            ):
                to_remove.add(prev_idx)
                # Check a single reasoning immediately before that computer_call
                r_idx = prev_idx - 1
                if r_idx >= 0 and messages[r_idx].get("type") == "reasoning":
                    to_remove.add(r_idx)

        # Construct filtered list
        filtered = [m for i, m in enumerate(messages) if i not in to_remove]
        return filtered
