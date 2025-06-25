"""
Utility functions for parsing and processing log files.
"""

import json
import os
from typing import Dict, List, Any, Optional, Tuple
from dataclasses import dataclass
from datetime import datetime


@dataclass
class Step:
    """Represents a single step in the task execution."""
    step_id: str
    tool_name: str
    tool_input: Dict[str, Any]
    tool_output: Optional[str]
    error: Optional[str]
    timestamp: str
    image_path: Optional[str] = None
    
    @property
    def is_successful(self) -> bool:
        """Check if this step was executed successfully."""
        return self.error is None
    
    @property
    def is_screenshot(self) -> bool:
        """Check if this step is a screenshot action."""
        return (self.tool_name == "computer" and 
                self.tool_input.get("action") == "screenshot")


@dataclass
class Task:
    """Represents a complete task execution."""
    session_id: str
    test_id: str
    category: str
    instruction: str
    model: str
    start_time: str
    end_time: str
    status: str
    steps: List[Step]
    total_cost: float
    total_tokens: int
    final_output: str
    
    @property
    def duration(self) -> float:
        """Calculate task duration in seconds."""
        try:
            if not self.start_time or not self.end_time:
                return 0.0
            start = datetime.fromisoformat(self.start_time)
            end = datetime.fromisoformat(self.end_time)
            return (end - start).total_seconds()
        except (ValueError, TypeError):
            return 0.0
    
    @property
    def screenshot_count(self) -> int:
        """Count number of screenshots taken."""
        return sum(1 for step in self.steps if step.is_screenshot)
    
    @property
    def action_count(self) -> int:
        """Count non-screenshot actions (actual work steps)."""
        return sum(1 for step in self.steps if not step.is_screenshot)


class LogParser:
    """Parser for structured JSON log files."""
    
    def __init__(self, logs_dir: str):
        self.logs_dir = logs_dir
    
    def get_log_files(self) -> List[str]:
        """Get all JSON log files in the logs directory."""
        if not os.path.exists(self.logs_dir):
            return []
        
        return [f for f in os.listdir(self.logs_dir) if f.endswith('.json')]
    
    def parse_log_file(self, filename: str) -> Optional[Task]:
        """Parse a single JSON log file into a Task object."""
        filepath = os.path.join(self.logs_dir, filename)
        
        try:
            with open(filepath, 'r') as f:
                data = json.load(f)
            
            # Extract metadata (may be missing in older logs)
            metadata = self._extract_metadata(data.get("events", []))
            if not metadata:
                # Create default metadata from filename or session_id
                session_id = data.get("session_id", filename.replace(".json", ""))
                metadata = {
                    "Test_ID": "unknown",
                    "Cat": "unknown"
                }
                print(f"Info: Created default metadata for {filename}")
            
            # Extract user instruction
            instruction = self._extract_instruction(data.get("events", []))
            
            # Extract model information
            model = self._extract_model(data.get("events", []))
            
            # Extract steps
            steps = self._extract_steps(data.get("events", []))
            
            # Extract final output
            final_output = self._extract_final_output(data.get("events", []))
            
            # Extract session summary (may be missing)
            session_summary = self._extract_session_summary(data.get("events", []))
            
            return Task(
                session_id=data.get("session_id", filename.replace(".json", "")),
                test_id=metadata.get("Test_ID", "unknown"),
                category=metadata.get("Cat", "unknown"),
                instruction=instruction,
                model=model,
                start_time=data.get("start_time", ""),
                end_time=data.get("end_time", ""),
                status=data.get("status", "unknown"),
                steps=steps,
                total_cost=session_summary.get("total_cost_usd", 0.0),
                total_tokens=session_summary.get("total_tokens", 0),
                final_output=final_output
            )
            
        except (json.JSONDecodeError, KeyError, Exception) as e:
            print(f"Error parsing {filename}: {e}")
            return None
    
    def _extract_metadata(self, events: List[Dict]) -> Optional[Dict]:
        """Extract session metadata from events."""
        for event in events:
            # Check for both metadata types for backward compatibility
            if event.get("type") in ["session_metadata", "test_metadata"]:
                return event.get("data", {})
        return None
    
    def _extract_instruction(self, events: List[Dict]) -> str:
        """Extract user instruction from events."""
        for event in events:
            if event.get("type") == "user_input":
                data = event.get("data", {})
                # Check for both "input" and "instruction" keys
                instruction = data.get("input", "") or data.get("instruction", "")
                if instruction:
                    return instruction
        return "No instruction found"
    
    def _extract_model(self, events: List[Dict]) -> str:
        """Extract model information from events."""
        for event in events:
            if event.get("type") == "model_selection":
                model = event.get("data", {}).get("model", "")
                if model:
                    return model
        return "unknown"
    
    def _extract_steps(self, events: List[Dict]) -> List[Step]:
        """Extract all execution steps from events."""
        steps = []
        tool_use_map = {}  # Map tool_use_id to tool info
        
        # First pass: collect all tool_use requests
        for event in events:
            if event.get("type") == "api_response":
                content = event.get("data", {}).get("content", [])
                if isinstance(content, list):
                    for block in content:
                        if (isinstance(block, dict) and 
                            block.get("type") == "tool_use"):
                            tool_use_map[block.get("id")] = {
                                "name": block.get("name"),
                                "input": block.get("input", {}),
                                "timestamp": event.get("timestamp")
                            }
        
        # Second pass: match tool results with tool uses
        for event in events:
            if event.get("type") == "tool_result":
                data = event.get("data", {})
                tool_use_id = data.get("tool_use_id")
                
                if tool_use_id and tool_use_id in tool_use_map:
                    tool_info = tool_use_map[tool_use_id]
                    
                    step = Step(
                        step_id=tool_use_id,
                        tool_name=tool_info["name"],
                        tool_input=tool_info["input"],
                        tool_output=data.get("output"),
                        error=data.get("error"),
                        timestamp=event.get("timestamp", ""),
                        image_path=data.get("image_path")
                    )
                    steps.append(step)
                elif tool_use_id:
                    # Tool result without matching tool_use (shouldn't happen but be safe)
                    step = Step(
                        step_id=tool_use_id,
                        tool_name="unknown",
                        tool_input={},
                        tool_output=data.get("output"),
                        error=data.get("error"),
                        timestamp=event.get("timestamp", ""),
                        image_path=data.get("image_path")
                    )
                    steps.append(step)
        
        return steps
    
    def _extract_final_output(self, events: List[Dict]) -> str:
        """Extract the final LLM output from events."""
        final_outputs = []
        
        for event in events:
            if event.get("type") == "api_response":
                content = event.get("data", {}).get("content", [])
                if isinstance(content, list):
                    for block in content:
                        if (isinstance(block, dict) and 
                            block.get("type") == "text"):
                            final_outputs.append(block.get("text", ""))
        
        # Return the last text output as final output
        return final_outputs[-1] if final_outputs else ""
    
    def _extract_session_summary(self, events: List[Dict]) -> Dict:
        """Extract session summary from events."""
        for event in events:
            if event.get("type") == "session_summary":
                return event.get("data", {})
        return {}
    
    def parse_all_logs(self) -> List[Task]:
        """Parse all log files in the directory."""
        log_files = self.get_log_files()
        tasks = []
        
        for filename in log_files:
            task = self.parse_log_file(filename)
            if task:
                tasks.append(task)
        
        return tasks


def get_screenshots_for_task(task: Task) -> List[Tuple[str, str]]:
    """Get all screenshots for a task as (path, timestamp) tuples."""
    screenshots = []
    for step in task.steps:
        if step.image_path and os.path.exists(step.image_path):
            screenshots.append((step.image_path, step.timestamp))
    return screenshots


def get_unique_task_id(task: Task) -> str:
    """Generate a unique task identifier combining test_id and session_id."""
    return f"{task.test_id}_{task.session_id}"


def format_task_display_name(task: Task) -> str:
    """Format a short, human-readable task display name."""
    # Extract timestamp from session_id if available
    session_parts = task.session_id.split('_')
    session_info = ""
    
    if len(session_parts) >= 3:
        timestamp_part = session_parts[-1]
        if len(timestamp_part) >= 8:  # YYYYMMDD format
            try:
                dt = datetime.strptime(timestamp_part[:8], '%Y%m%d')
                date_str = dt.strftime('%m/%d')
                time_str = timestamp_part[9:] if len(timestamp_part) > 8 else ""
                if time_str and len(time_str) >= 6:
                    formatted_time = f"{time_str[:2]}:{time_str[2:4]}"
                    session_info = f" ({date_str} {formatted_time})"
                else:
                    session_info = f" ({date_str})"
            except:
                session_info = f" ({timestamp_part})"
    
    return f"{task.test_id}{session_info}"


def format_task_summary(task: Task) -> str:
    """Format a task summary for display."""
    # Extract and format timestamp from session_id
    session_parts = task.session_id.split('_')
    session_info = task.session_id
    
    if len(session_parts) >= 3:
        timestamp_part = session_parts[-1]
        if len(timestamp_part) >= 8:  # YYYYMMDD format
            try:
                dt = datetime.strptime(timestamp_part[:8], '%Y%m%d')
                date_str = dt.strftime('%Y-%m-%d')
                time_str = timestamp_part[9:] if len(timestamp_part) > 8 else ""
                if time_str and len(time_str) >= 6:
                    formatted_time = f"{time_str[:2]}:{time_str[2:4]}:{time_str[4:6]}"
                    session_info = f"{date_str} {formatted_time}"
                else:
                    session_info = date_str
            except:
                session_info = task.session_id
    
    return f"""
Session ID: {task.session_id}
Session Time: {session_info}
Task ID: {task.test_id}
Category: {task.category}
Instruction: {task.instruction}
Duration: {task.duration:.1f}s
Status: {task.status}
Steps: {len(task.steps)} total ({task.action_count} actions, {task.screenshot_count} screenshots)
Cost: ${task.total_cost:.6f}
Model: {task.model}
""".strip()
