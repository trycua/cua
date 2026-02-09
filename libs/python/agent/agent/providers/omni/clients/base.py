"""Base client implementation for Omni providers."""

import logging
from typing import Any, Dict, List, Optional, Tuple

logger = logging.getLogger(__name__)


class BaseOmniClient:
    """Base class for provider-specific clients."""

    def __init__(self, api_key: Optional[str] = None, model: Optional[str] = None):
        """Initialize base client.

        Args:
            api_key: Optional API key
            model: Optional model name
        """
        self.api_key = api_key
        self.model = model

    async def run_interleaved(
        self, messages: List[Dict[str, Any]], system: str, max_tokens: Optional[int] = None
    ) -> Dict[str, Any]:
        """Run interleaved chat completion.

        Args:
            messages: List of message dicts
            system: System prompt
            max_tokens: Optional max tokens override

        Returns:
            Response dict
        """
        raise NotImplementedError
