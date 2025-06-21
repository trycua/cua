import streamlit as st
import asyncio
import os
import json
import base64
import logging
import datetime
from dotenv import load_dotenv
from json_logger import JSONLogger
from collections import Counter

# Load environment variables from .env file
load_dotenv()

from computer_use_demo.loop import sampling_loop, APIProvider
from computer_use_demo.tools import ToolResult
from anthropic.types.beta import BetaMessage, BetaMessageParam
from anthropic import APIResponse

# Set up logging
logs_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "logs")
os.makedirs(logs_dir, exist_ok=True)
log_filename = os.path.join(logs_dir, f"claude_computer_use_{datetime.datetime.now().strftime('%Y%m%d_%H%M%S')}.log")

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s [%(levelname)s] %(message)s',
    handlers=[
        logging.FileHandler(log_filename),
        logging.StreamHandler()  # Also log to console
    ]
)
logger = logging.getLogger("claude_computer_use")

# Define the Streamlit app layout and functionality
def main():
    st.title("Claude Computer Use Demo")
    st.write("A modern, high-end, professional interface for interacting with Claude.")
    
    # Initialize the JSON logger in session state to persist it across reruns
    if 'json_logger' not in st.session_state:
        session_id = datetime.datetime.now().strftime('%Y%m%d_%H%M%S')
        st.session_state.json_logger = JSONLogger(log_dir=logs_dir, session_id=session_id)
        logger.info(f"Created new JSON logger with session ID: {session_id}")
    
    # Log application start
    logger.info("Streamlit application started")

    # Set up your Anthropic API key and model
    api_key = os.getenv("ANTHROPIC_API_KEY", "YOUR_API_KEY_HERE")
    if api_key == "YOUR_API_KEY_HERE":
        error_msg = "API key not found in environment variables"
        logger.error(error_msg)
        st.error("Please set your API key in the ANTHROPIC_API_KEY environment variable.")
        return
    
    logger.info("API key successfully loaded")
    provider = APIProvider.ANTHROPIC

    # User input section
    instruction = st.text_input("Enter your instruction:", "Save an image of a cat to the desktop.")
    logger.info(f"User entered instruction: {instruction}")
    st.session_state.json_logger.log_user_input(instruction)

    if st.button("Run"):
        st.write(f"Starting Claude 'Computer Use' with instruction: '{instruction}'")
        logger.info(f"Run button clicked with instruction: '{instruction}'")

        # Set up the initial messages
        messages: list[BetaMessageParam] = [
            {
                "role": "user",
                "content": instruction,
            }
        ]
        logger.info(f"Messages initialized: {json.dumps(messages)}")

        # Define callbacks
        def output_callback(content_block):
            if isinstance(content_block, dict) and content_block.get("type") == "text":
                text = content_block.get("text", "")
                st.write("Assistant:", text)
                logger.info(f"Assistant output: {text}")
                st.session_state.json_logger.log_assistant_output(text)
            else:
                logger.info(f"Non-text content block: {content_block}")

        def tool_output_callback(result: ToolResult, tool_use_id: str):
            if result.output:
                output_text = f"> Tool Output [{tool_use_id}]: {result.output}"
                st.write(output_text)
                logger.info(output_text)
                st.session_state.json_logger.log_tool_result(tool_use_id, output=result.output)
            if result.error:
                error_text = f"!!! Tool Error [{tool_use_id}]: {result.error}"
                st.write(error_text)
                logger.error(error_text)
                st.session_state.json_logger.log_tool_result(tool_use_id, error=result.error)
            if result.base64_image:
                # Save the image to a file if needed
                os.makedirs("screenshots", exist_ok=True)
                image_data = result.base64_image
                image_path = f"screenshots/screenshot_{tool_use_id}.png"
                with open(image_path, "wb") as f:
                    f.write(base64.b64decode(image_data))
                st.image(image_path, caption=f"Screenshot {tool_use_id}")
                logger.info(f"Screenshot saved: {image_path}")
                st.session_state.json_logger.log_tool_result(tool_use_id, image_path=image_path)

        def api_response_callback(response: APIResponse[BetaMessage]):
            response_content = json.loads(response.text)["content"]  # type: ignore
            response_text = json.dumps(response_content, indent=4)
            st.write(
                "\n---------------\nAPI Response:\n",
                response_text,
                "\n",
            )
            logger.info(f"API Response: {response_text}")
            st.session_state.json_logger.log_api_response(response_content)

        # Run the sampling loop
        model_name = "claude-3-5-sonnet-20241022"
        logger.info(f"Starting sampling loop with model: {model_name}")
        st.session_state.json_logger.log_event("model_selection", {"model": model_name})
        try:
            asyncio.run(sampling_loop(
                model=model_name,
                provider=provider,
                system_prompt_suffix="",
                messages=messages,
                output_callback=output_callback,
                tool_output_callback=tool_output_callback,
                api_response_callback=api_response_callback,
                api_key=api_key,
                only_n_most_recent_images=10,
                max_tokens=4096,
            ))
            logger.info("Sampling loop completed successfully")
            st.session_state.json_logger.finalize(status="completed")
        except Exception as e:
            error_msg = f"Error during sampling loop: {str(e)}"
            logger.error(error_msg)
            st.error(error_msg)
            st.session_state.json_logger.log_error(error_msg, exception_type=type(e).__name__)
            st.session_state.json_logger.finalize(status="error")
            
    # Add a section to display log file contents
    st.sidebar.title("Logs")
    if os.path.exists(logs_dir):
        # Get both text and JSON logs
        log_files = [f for f in os.listdir(logs_dir) if f.endswith('.log')]
        json_files = [f for f in os.listdir(logs_dir) if f.endswith('.json')]
        
        log_files.sort(reverse=True)  # Show newest logs first
        json_files.sort(reverse=True)  # Show newest logs first
        
        log_tab, json_tab = st.sidebar.tabs(["Text Logs", "JSON Logs"])
        
        # Text logs tab
        with log_tab:
            if log_files:
                selected_log = st.selectbox("Select a text log file to view:", log_files, key="text_log")
                if selected_log:
                    log_path = os.path.join(logs_dir, selected_log)
                    with open(log_path, 'r') as f:
                        log_content = f.read()
                    
                    if st.checkbox("Show log file content", key="show_text_log"):
                        st.text_area("Log Content", log_content, height=400)
                    
                    # Add option to download the log file
                    with open(log_path, "rb") as f:
                        st.download_button(
                            label="Download Log File",
                            data=f,
                            file_name=selected_log,
                            mime="text/plain",
                            key="download_text_log"
                        )
            else:
                st.info("No text log files found.")
        
        # JSON logs tab
        with json_tab:
            if json_files:
                selected_json = st.selectbox("Select a JSON log file to view:", json_files, key="json_log")
                if selected_json:
                    json_path = os.path.join(logs_dir, selected_json)
                    with open(json_path, 'r') as f:
                        try:
                            json_content = json.load(f)
                        except json.JSONDecodeError:
                            json_content = {"error": "Invalid JSON file"}
                    
                    if st.checkbox("Show JSON log content", key="show_json_log"):
                        st.json(json_content)
                    
                    # Add option to download the JSON file
                    with open(json_path, "rb") as f:
                        st.download_button(
                            label="Download JSON Log File",
                            data=f,
                            file_name=selected_json,
                            mime="application/json",
                            key="download_json_log"
                        )
                    
                    # Add option to analyze the log
                    if st.button("Analyze this log", key="analyze_json_log"):
                        st.sidebar.markdown("### Log Analysis")
                        # Get basic stats
                        event_counts = Counter([event.get("type") for event in json_content.get("events", [])])
                        st.sidebar.write("Event counts:")
                        for event_type, count in event_counts.items():
                            st.sidebar.write(f"- {event_type}: {count}")
                        
                        # Get session duration
                        if "start_time" in json_content and "end_time" in json_content:
                            start = datetime.datetime.fromisoformat(json_content["start_time"])
                            end = datetime.datetime.fromisoformat(json_content["end_time"])
                            duration = (end - start).total_seconds()
                            st.sidebar.write(f"Session duration: {duration:.2f} seconds")
            else:
                st.info("No JSON log files found.")

if __name__ == "__main__":
    main()
