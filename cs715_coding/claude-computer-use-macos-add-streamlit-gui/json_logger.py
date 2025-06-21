import json
import os
import datetime
import threading
from typing import Dict, List, Any, Optional

class JSONLogger:
    """
    A logger that stores all events in a structured JSON format for easy analysis and evaluation.
    Each event is stored with a timestamp, event type, and relevant data.
    """
    
    def __init__(self, log_dir="logs", session_id=None):
        """Initialize the JSON Logger with a directory for logs"""
        self.log_dir = log_dir
        os.makedirs(self.log_dir, exist_ok=True)
        
        # Generate a session ID based on timestamp if not provided
        self.session_id = session_id or datetime.datetime.now().strftime('%Y%m%d_%H%M%S')
        
        # Create the log file path
        self.log_file = os.path.join(self.log_dir, f"session_{self.session_id}.json")
        
        # Initialize the log data structure
        self.log_data = {
            "session_id": self.session_id,
            "start_time": datetime.datetime.now().isoformat(),
            "events": []
        }
        
        # Save initial log structure
        self._save_log()
        
        # Thread lock for thread safety
        self.lock = threading.Lock()
    
    def _save_log(self):
        """Save the current log data to the JSON file"""
        with open(self.log_file, 'w') as f:
            json.dump(self.log_data, f, indent=2)
    
    def log_event(self, event_type: str, data: Dict[str, Any], metadata: Optional[Dict[str, Any]] = None):
        """
        Log an event with the given type and data
        
        Args:
            event_type: The type of event (e.g., 'user_input', 'tool_call', 'assistant_response')
            data: The main data associated with the event
            metadata: Optional additional metadata about the event
        """
        with self.lock:
            event = {
                "timestamp": datetime.datetime.now().isoformat(),
                "type": event_type,
                "data": data
            }
            
            if metadata:
                event["metadata"] = metadata
                
            self.log_data["events"].append(event)
            self._save_log()
    
    def log_user_input(self, instruction: str):
        """Log a user input event"""
        self.log_event("user_input", {"instruction": instruction})
    
    def log_assistant_output(self, text: str):
        """Log an assistant output event"""
        self.log_event("assistant_output", {"text": text})
    
    def log_tool_call(self, tool_name: str, tool_input: Any, tool_use_id: str):
        """Log a tool call event"""
        self.log_event("tool_call", {
            "tool_name": tool_name,
            "tool_input": tool_input,
            "tool_use_id": tool_use_id
        })
    
    def log_tool_result(self, tool_use_id: str, output: Optional[str] = None, 
                       error: Optional[str] = None, image_path: Optional[str] = None):
        """Log a tool result event"""
        data = {"tool_use_id": tool_use_id}
        if output:
            data["output"] = output
        if error:
            data["error"] = error
        if image_path:
            data["image_path"] = image_path
            
        self.log_event("tool_result", data)
    
    def log_api_response(self, response_content: Any):
        """Log an API response event"""
        self.log_event("api_response", {"content": response_content})
    
    def log_error(self, error_message: str, exception_type: Optional[str] = None):
        """Log an error event"""
        data = {"error_message": error_message}
        if exception_type:
            data["exception_type"] = exception_type
            
        self.log_event("error", data)
    
    def finalize(self, status: str = "completed", metadata: Optional[Dict[str, Any]] = None):
        """Finalize the log with completion information"""
        with self.lock:
            self.log_data["end_time"] = datetime.datetime.now().isoformat()
            self.log_data["status"] = status
            
            if metadata:
                self.log_data["completion_metadata"] = metadata
                
            self._save_log()
    
    def get_log_file_path(self):
        """Return the path to the log file"""
        return self.log_file
