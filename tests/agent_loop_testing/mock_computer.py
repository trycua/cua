"""
Minimal Mock Computer for Agent Loop Testing

This module provides a simplified mock computer that only implements
screenshot functionality, returning a static image for testing AI models
with the agent loop.
"""

import asyncio
import base64
import os
from pathlib import Path
from typing import Dict, Any, Optional
from PIL import Image, ImageDraw
from io import BytesIO


class MinimalMockComputer:
    """
    A minimal mock computer that only provides screenshot functionality.
    
    This is designed to test AI models with the agent loop without the
    complexity of a full computer implementation.
    """
    
    def __init__(self, image_path: Optional[str] = None):
        """
        Initialize the mock computer.
        
        Args:
            image_path: Path to a static image file. If None, creates a default image.
        """
        self.image_path = image_path
        self.action_count = 0
        self.static_image = None
        self._initialized = False
    
    async def initialize(self) -> None:
        """Initialize the mock computer by loading the static image."""
        if self._initialized:
            return
            
        if self.image_path and os.path.exists(self.image_path):
            with open(self.image_path, 'rb') as f:
                self.static_image = base64.b64encode(f.read()).decode('utf-8')
        else:
            # Create a default macOS desktop image
            self.static_image = self._create_default_image()
        
        self._initialized = True
    
    def _create_default_image(self) -> str:
        """Create a default macOS desktop image."""
        img = Image.new('RGB', (1920, 1080), color='lightblue')
        draw = ImageDraw.Draw(img)
        
        # Draw macOS-style desktop
        draw.rectangle([0, 0, 1920, 1080], fill='lightblue')
        
        # Draw Safari icon in dock area
        draw.rectangle([100, 950, 150, 1000], fill='blue', outline='black', width=2)
        draw.text((110, 960), "Safari", fill='white')
        
        # Add some additional UI elements for testing
        draw.rectangle([200, 950, 250, 1000], fill='green', outline='black', width=2)
        draw.text((210, 960), "Terminal", fill='white')
        
        draw.rectangle([300, 950, 350, 1000], fill='red', outline='black', width=2)
        draw.text((310, 960), "Finder", fill='white')
        
        # Convert to base64
        img_bytes = BytesIO()
        img.save(img_bytes, format='PNG')
        return base64.b64encode(img_bytes.getvalue()).decode('utf-8')
    
    async def screenshot(self) -> str:
        """
        Take a screenshot and return the static image as base64.
        
        Returns:
            Base64-encoded PNG image string
        """
        if not self._initialized:
            await self.initialize()
        
        self.action_count += 1
        return self.static_image
    
    async def get_screen_dimensions(self) -> tuple[int, int]:
        """Get screen dimensions as (width, height)."""
        return (1920, 1080)
    
    def get_action_count(self) -> int:
        """Get the number of screenshot actions performed."""
        return self.action_count
    
    def reset_action_count(self) -> None:
        """Reset the action counter."""
        self.action_count = 0


class MockComputerInterface:
    """
    Interface for AI models to interact with the mock computer.
    
    This provides a clean interface that AI models can use to interact
    with the mock computer, abstracting away implementation details.
    """
    
    def __init__(self, mock_computer: MinimalMockComputer):
        self.computer = mock_computer
    
    async def take_screenshot(self) -> str:
        """Take a screenshot and return it as base64."""
        return await self.computer.screenshot()
    
    async def get_screen_info(self) -> Dict[str, Any]:
        """Get information about the screen."""
        width, height = await self.computer.get_screen_dimensions()
        return {
            "width": width,
            "height": height,
            "action_count": self.computer.get_action_count()
        }
    
    def reset_stats(self) -> None:
        """Reset action statistics."""
        self.computer.reset_action_count()
