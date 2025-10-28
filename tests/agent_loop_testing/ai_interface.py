"""
Pluggable AI Model Interface for Agent Loop Testing

This module provides interfaces and base classes for plugging in different
AI models to test with the agent loop using a minimal mock computer.
"""

import asyncio
from abc import ABC, abstractmethod
from typing import Dict, Any, List, Optional, AsyncGenerator
from dataclasses import dataclass
try:
    from .mock_computer import MockComputerInterface
except ImportError:
    # When running as script directly
    from mock_computer import MockComputerInterface


@dataclass
class AgentMessage:
    """Represents a message in the agent conversation."""
    role: str  # "user", "assistant", "system"
    content: str
    image_data: Optional[str] = None  # Base64 encoded image


@dataclass
class AgentResponse:
    """Represents a response from the agent."""
    content: str
    tool_calls: List[Dict[str, Any]]
    finished: bool = False


class AIModelInterface(ABC):
    """
    Abstract interface for AI models that can be plugged into the agent loop.
    
    This allows any AI model to be tested with the agent loop by implementing
    this interface.
    """
    
    @abstractmethod
    async def generate_response(
        self, 
        messages: List[AgentMessage],
        computer_interface: MockComputerInterface
    ) -> AgentResponse:
        """
        Generate a response from the AI model.
        
        Args:
            messages: List of conversation messages
            computer_interface: Interface to interact with the mock computer
            
        Returns:
            AgentResponse containing the model's response
        """
        pass
    
    @abstractmethod
    def get_model_name(self) -> str:
        """Get the name/identifier of the AI model."""
        pass


class AgentLoop:
    """
    The agent loop that coordinates between an AI model and the mock computer.
    
    This is the core component that runs the agent loop, allowing any AI model
    to be plugged in for testing.
    """
    
    def __init__(self, ai_model: AIModelInterface, computer_interface: MockComputerInterface):
        self.ai_model = ai_model
        self.computer = computer_interface
        self.conversation_history: List[AgentMessage] = []
        self.max_iterations = 10
        self.iteration_count = 0
    
    async def run(
        self, 
        initial_message: str,
        max_iterations: Optional[int] = None
    ) -> AsyncGenerator[Dict[str, Any], None]:
        """
        Run the agent loop with the given initial message.
        
        Args:
            initial_message: The initial user message to start the conversation
            max_iterations: Maximum number of iterations (defaults to self.max_iterations)
            
        Yields:
            Dict containing iteration results and agent responses
        """
        if max_iterations is not None:
            self.max_iterations = max_iterations
        
        self.iteration_count = 0
        self.conversation_history = []
        
        # Add initial user message
        self.conversation_history.append(AgentMessage(
            role="user",
            content=initial_message
        ))
        
        while self.iteration_count < self.max_iterations:
            self.iteration_count += 1
            
            # Take a screenshot
            screenshot = await self.computer.take_screenshot()
            
            # Add screenshot to the last user message or create a new one
            if self.conversation_history and self.conversation_history[-1].role == "user":
                self.conversation_history[-1].image_data = screenshot
            else:
                self.conversation_history.append(AgentMessage(
                    role="user",
                    content="Here's the current screen:",
                    image_data=screenshot
                ))
            
            # Generate AI response
            try:
                response = await self.ai_model.generate_response(
                    self.conversation_history,
                    self.computer
                )
                
                # Add AI response to conversation history
                self.conversation_history.append(AgentMessage(
                    role="assistant",
                    content=response.content
                ))
                
                # Yield iteration result
                yield {
                    "iteration": self.iteration_count,
                    "response": response,
                    "screenshot_taken": True,
                    "conversation_length": len(self.conversation_history)
                }
                
                # Check if agent is finished
                if response.finished:
                    break
                    
            except Exception as e:
                yield {
                    "iteration": self.iteration_count,
                    "error": str(e),
                    "screenshot_taken": True,
                    "conversation_length": len(self.conversation_history)
                }
                break
    
    def get_conversation_history(self) -> List[AgentMessage]:
        """Get the full conversation history."""
        return self.conversation_history.copy()
    
    def reset(self) -> None:
        """Reset the agent loop state."""
        self.conversation_history = []
        self.iteration_count = 0
        self.computer.reset_stats()


class SimpleTestAIModel(AIModelInterface):
    """
    A simple test AI model for demonstration purposes.
    
    This model provides basic responses and can be used to test the
    agent loop without requiring external AI services.
    """
    
    def __init__(self, model_name: str = "test-model"):
        self.model_name = model_name
        self.response_count = 0
    
    async def generate_response(
        self, 
        messages: List[AgentMessage],
        computer_interface: MockComputerInterface
    ) -> AgentResponse:
        """Generate a simple test response."""
        self.response_count += 1
        
        # Get screen info
        screen_info = await computer_interface.get_screen_info()
        
        # Simple logic: respond based on iteration count
        if self.response_count == 1:
            content = "I can see a macOS desktop with Safari, Terminal, and Finder icons in the dock. Let me click on Safari to open it."
            tool_calls = [{"name": "click", "args": {"x": 125, "y": 975}}]
        elif self.response_count == 2:
            content = "I clicked on Safari but it didn't open. Let me try clicking on Terminal instead."
            tool_calls = [{"name": "click", "args": {"x": 225, "y": 975}}]
        elif self.response_count == 3:
            content = "I've tried clicking on different icons but nothing seems to be happening. This might be a static image. Let me finish here."
            tool_calls = []
        else:
            content = f"This is iteration {self.response_count}. I can see the screen has dimensions {screen_info['width']}x{screen_info['height']}."
            tool_calls = []
        
        return AgentResponse(
            content=content,
            tool_calls=tool_calls,
            finished=self.response_count >= 3
        )
    
    def get_model_name(self) -> str:
        return self.model_name
