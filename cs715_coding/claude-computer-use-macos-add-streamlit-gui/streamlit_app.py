import streamlit as st
import asyncio
import os
import json
import base64
import datetime
import pandas as pd
import plotly.express as px
import plotly.graph_objects as go
from PIL import Image
from dotenv import load_dotenv
from collections import Counter
from computer_use_demo.loop import sampling_loop, APIProvider
from computer_use_demo.tools import ToolResult
from anthropic.types.beta import BetaMessage, BetaMessageParam
from anthropic import APIResponse
from json_logger import JSONLogger

# Claude pricing per 1K tokens (as of 2025)
CLAUDE_PRICING = {
    "claude-3-7-sonnet-20250219": {
        "input": 0.003,   # $3 per million input tokens
        "output": 0.015,  # $15 per million output tokens
    },
    "claude-3-5-sonnet-20241022": {
        "input": 0.003,   # $3 per million input tokens  
        "output": 0.015,  # $15 per million output tokens
    },
    "claude-3-opus-20240229": {
        "input": 0.015,   # $15 per million input tokens
        "output": 0.075,  # $75 per million output tokens
    },
}

def calculate_cost(model_name: str, input_tokens: int, output_tokens: int) -> float:
    """Calculate cost based on token usage and model pricing.
    
    Args:
        model_name: Name of the Claude model
        input_tokens: Number of input tokens
        output_tokens: Number of output tokens
        
    Returns:
        Total cost in USD
    """
    if model_name not in CLAUDE_PRICING:
        return 0.0
    
    pricing = CLAUDE_PRICING[model_name]
    input_cost = (input_tokens / 1000) * pricing["input"]
    output_cost = (output_tokens / 1000) * pricing["output"]
    
    return input_cost + output_cost

def analyze_log_page(json_content, selected_json):
    """Display comprehensive log analysis with visualizations and screenshots."""
    st.header("📊 Log Analysis Dashboard")
    st.write(f"**Analysis for:** `{selected_json}`")
    
    # Extract metadata
    metadata = None
    for event in json_content.get("events", []):
        # Check for both metadata types for backward compatibility
        if event.get("type") in ["session_metadata", "test_metadata"]:
            metadata = event.get("data", {})
            break
    
    # Display session overview
    col1, col2, col3 = st.columns(3)
    
    with col1:
        if metadata:
            st.metric("Test ID", metadata.get("Test_ID", "N/A"))
            st.metric("Category", metadata.get("Cat", "N/A"))
    
    with col2:
        if "start_time" in json_content and "end_time" in json_content:
            start = datetime.datetime.fromisoformat(json_content["start_time"])
            end = datetime.datetime.fromisoformat(json_content["end_time"])
            duration = (end - start).total_seconds()
            st.metric("Duration", f"{duration:.1f}s")
            st.metric("Status", json_content.get("status", "N/A"))
    
    with col3:
        # Find session summary for cost info
        session_summary = None
        for event in json_content.get("events", []):
            if event.get("type") == "session_summary":
                session_summary = event.get("data", {})
                break
        
        if session_summary:
            st.metric("Total Cost", f"${session_summary.get('total_cost_usd', 0):.6f}")
            st.metric("Total Tokens", f"{session_summary.get('total_tokens', 0):,}")
    
    # Event analysis
    st.subheader("📈 Event Analysis")
    events = json_content.get("events", [])
    event_counts = Counter([event.get("type") for event in events])
    
    if event_counts:
        # Create event distribution chart
        event_df = pd.DataFrame(list(event_counts.items()), columns=['Event Type', 'Count'])
        fig_events = px.bar(event_df, x='Event Type', y='Count', 
                           title="Event Distribution",
                           color='Count',
                           color_continuous_scale='viridis')
        fig_events.update_layout(xaxis_tickangle=-45)
        st.plotly_chart(fig_events, use_container_width=True)
    
    # Token usage over time
    st.subheader("💰 Token Usage & Cost Analysis")
    token_events = [e for e in events if e.get("type") == "token_usage"]
    
    if token_events:
        token_data = []
        cumulative_cost = 0
        cumulative_tokens = 0
        
        for i, event in enumerate(token_events):
            data = event.get("data", {})
            cumulative_cost += data.get("cost_usd", 0)
            cumulative_tokens += data.get("input_tokens", 0) + data.get("output_tokens", 0)
            
            token_data.append({
                "Request": i + 1,
                "Input Tokens": data.get("input_tokens", 0),
                "Output Tokens": data.get("output_tokens", 0),
                "Cost": data.get("cost_usd", 0),
                "Cumulative Cost": cumulative_cost,
                "Cumulative Tokens": cumulative_tokens
            })
        
        token_df = pd.DataFrame(token_data)
        
        # Token usage chart
        fig_tokens = go.Figure()
        fig_tokens.add_trace(go.Bar(name='Input Tokens', x=token_df['Request'], y=token_df['Input Tokens']))
        fig_tokens.add_trace(go.Bar(name='Output Tokens', x=token_df['Request'], y=token_df['Output Tokens']))
        fig_tokens.update_layout(title="Token Usage per Request", barmode='stack')
        st.plotly_chart(fig_tokens, use_container_width=True)
        
        # Cumulative cost chart
        fig_cost = px.line(token_df, x='Request', y='Cumulative Cost', 
                          title="Cumulative Cost Over Time",
                          markers=True)
        fig_cost.update_layout(yaxis_title="Cost ($)")
        st.plotly_chart(fig_cost, use_container_width=True)
        
        # Token usage table
        st.dataframe(token_df, use_container_width=True)
    
    # Screenshots section
    st.subheader("📸 Screenshots & Context Flow")
    
    # Look for screenshot references in tool results and build context
    screenshot_events = []
    
    for i, event in enumerate(events):
        if event.get("type") == "tool_result":
            data = event.get("data", {})
            if "image_path" in data:
                # Find the API call that requested this screenshot
                requesting_api_call = None
                
                # Look backwards to find the API response that requested this screenshot
                for j in range(i-1, max(0, i-5), -1):  # Look at up to 5 preceding events
                    prev_event = events[j]
                    if prev_event.get("type") == "api_response":
                        # Check if this API response contains the tool_use that matches our tool_use_id
                        api_content = prev_event.get("data", {}).get("content", [])
                        if isinstance(api_content, list):
                            for content_block in api_content:
                                if (isinstance(content_block, dict) and 
                                    content_block.get("type") == "tool_use" and
                                    content_block.get("id") == data.get("tool_use_id")):
                                    requesting_api_call = prev_event
                                    break
                        if requesting_api_call:
                            break
                
                # Find subsequent events that use this screenshot as input
                subsequent_events = []
                
                # Look forward to find events that happen after this screenshot
                for j in range(i+1, min(len(events), i+10)):  # Look at up to 10 following events
                    next_event = events[j]
                    event_type = next_event.get("type")
                    
                    if event_type in ["api_response", "token_usage"]:
                        subsequent_events.append(next_event)
                        # Stop at the next screenshot to avoid mixing contexts
                        if (event_type == "tool_result" and 
                            "image_path" in next_event.get("data", {})):
                            break
                
                screenshot_events.append({
                    "timestamp": event.get("timestamp"),
                    "tool_use_id": data.get("tool_use_id"),
                    "image_path": data.get("image_path"),
                    "output": data.get("output", ""),
                    "requesting_api_call": requesting_api_call,
                    "subsequent_events": subsequent_events
                })
    
    if screenshot_events:
        for i, screenshot in enumerate(screenshot_events):
            with st.expander(f"Screenshot {i+1}: {screenshot['tool_use_id']}", expanded=i==0):
                
                # Show what triggered this screenshot
                st.subheader("� Screenshot Request")
                if screenshot["requesting_api_call"]:
                    req_data = screenshot["requesting_api_call"].get("data", {})
                    req_timestamp = screenshot["requesting_api_call"].get("timestamp", "")
                    st.markdown(f"**⚡ API Response** _{req_timestamp}_")
                    
                    # Show the context that led to requesting this screenshot
                    api_content = req_data.get("content", [])
                    if isinstance(api_content, list):
                        for content_block in api_content:
                            if isinstance(content_block, dict):
                                if content_block.get("type") == "text":
                                    text_content = content_block.get("text", "")
                                    if text_content:
                                        st.success(f"💭 {text_content}")
                                elif content_block.get("type") == "tool_use":
                                    tool_name = content_block.get("name", "unknown")
                                    tool_input = content_block.get("input", {})
                                    if tool_name == "computer" and tool_input.get("action") == "screenshot":
                                        st.info("🔧 **Screenshot requested** to capture current state")
                                    else:
                                        st.warning(f"🔧 Tool Call: **{tool_name}** - {str(tool_input)}")
                else:
                    st.info("Could not find the API call that requested this screenshot.")
                
                # Show the actual screenshot (the input for next model call)
                st.subheader("📸 Captured Screenshot (Model Input)")
                col1, col2 = st.columns([2, 1])
                
                with col1:
                    image_path = screenshot["image_path"]
                    if os.path.exists(image_path):
                        try:
                            from PIL import Image
                            image = Image.open(image_path)
                            st.image(image, caption=f"Screenshot captured: {screenshot['tool_use_id']}")
                        except Exception as e:
                            st.error(f"Could not load image: {e}")
                    else:
                        st.warning(f"Image file not found: {image_path}")
                
                with col2:
                    st.write("**Timestamp:**", screenshot["timestamp"])
                    st.write("**Tool Use ID:**", screenshot["tool_use_id"])
                    if screenshot["output"]:
                        st.write("**Tool Output:**")
                        st.code(screenshot["output"])
                
                # Show how the model responded after seeing this screenshot
                st.subheader("📥 Model Response After Seeing Screenshot")
                if screenshot["subsequent_events"]:
                    for sub_event in screenshot["subsequent_events"]:
                        event_type = sub_event.get("type")
                        timestamp = sub_event.get("timestamp", "")
                        data = sub_event.get("data", {})
                        
                        if event_type == "api_response":
                            st.markdown(f"**⚡ API Response** _{timestamp}_")
                            api_content = data.get("content", [])
                            if isinstance(api_content, list):
                                for content_block in api_content:
                                    if isinstance(content_block, dict):
                                        if content_block.get("type") == "text":
                                            text_content = content_block.get("text", "")
                                            if text_content:
                                                st.success(f"💭 {text_content}")
                                        elif content_block.get("type") == "tool_use":
                                            tool_name = content_block.get("name", "unknown")
                                            tool_input = content_block.get("input", {})
                                            st.warning(f"🔧 Next Action: **{tool_name}** - {str(tool_input)}")
                        
                        elif event_type == "token_usage":
                            tokens = data
                            cost = tokens.get("cost_usd", 0)
                            st.info(f"💰 Tokens: {tokens.get('input_tokens', 0)} in / {tokens.get('output_tokens', 0)} out | Cost: ${cost:.6f}")
                else:
                    st.info("No subsequent model responses found (possibly end of session).")
    else:
        st.info("No screenshots found in this session.")
    
    # Timeline view
    st.subheader("⏱️ Event Timeline")
    
    timeline_events = []
    for event in events:
        event_type = event.get("type", "unknown")
        timestamp = event.get("timestamp", "")
        
        # Create readable descriptions
        description = ""
        if event_type == "user_input":
            description = f"User: {event.get('data', {}).get('input', '')[:100]}..."
        elif event_type == "assistant_output":
            description = f"Assistant: {event.get('data', {}).get('output', '')[:100]}..."
        elif event_type == "tool_result":
            tool_id = event.get('data', {}).get('tool_use_id', 'unknown')
            description = f"Tool Result [{tool_id}]"
        elif event_type == "token_usage":
            tokens = event.get('data', {})
            description = f"Tokens: {tokens.get('input_tokens', 0)} in / {tokens.get('output_tokens', 0)} out"
        else:
            description = event_type.replace("_", " ").title()
        
        timeline_events.append({
            "Time": timestamp,
            "Event": event_type,
            "Description": description
        })
    
    if timeline_events:
        timeline_df = pd.DataFrame(timeline_events)
        st.dataframe(timeline_df, use_container_width=True)
    
    # Raw JSON view
    with st.expander("🔍 Raw JSON Data"):
        st.json(json_content)

def analyze_logs_main():
    """Main function for the log analysis page."""
    st.title("📊 Log Analysis Dashboard")
    st.write("Comprehensive analysis of your Claude Computer Use sessions with visualizations and screenshots.")
    
    # Get JSON logs
    if os.path.exists(logs_dir):
        json_files = [f for f in os.listdir(logs_dir) if f.endswith('.json')]
        json_files.sort(reverse=True)  # Show newest logs first
        
        if json_files:
            selected_json = st.selectbox(
                "Select a log file to analyze:", 
                json_files,
                key="analyze_json_select"
            )
            
            if selected_json:
                json_path = os.path.join(logs_dir, selected_json)
                with open(json_path, 'r') as f:
                    try:
                        json_content = json.load(f)
                        analyze_log_page(json_content, selected_json)
                    except json.JSONDecodeError:
                        st.error("Invalid JSON file format")
        else:
            st.info("No log files found. Run some experiments first!")
    else:
        st.error("Logs directory not found.")

# Load environment variables from .env file
load_dotenv()

# Set up logs directory for JSON logs only
logs_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "logs")
os.makedirs(logs_dir, exist_ok=True)

# Define the Streamlit app layout and functionality
def main():
    st.title("Claude Computer Use MacOS")
    st.write("This is the part of Seminar CS 715 (FSS2025) at University of Mannheim, Germany.")

    # Add page navigation
    page = st.sidebar.selectbox("Select Page", ["Run Experiment", "Analyze Logs", "Evaluate Tasks"])
    
    if page == "Analyze Logs":
        analyze_logs_main()
        return
    elif page == "Evaluate Tasks":
        # Import and run evaluation interface
        try:
            from run_eval import evaluation_interface
            evaluation_interface()
        except ImportError as e:
            st.error(f"Could not load evaluation interface: {e}")
        return

    # Add auto-scroll JavaScript
    auto_scroll_js = """
    <script>
    function autoScroll() {
        window.scrollTo(0, document.body.scrollHeight);
    }
    // Auto-scroll when new content is added
    setTimeout(autoScroll, 100);
    </script>
    """

    # Set up your Anthropic API key and model
    api_key = os.getenv("ANTHROPIC_API_KEY", "YOUR_API_KEY_HERE")
    if api_key == "YOUR_API_KEY_HERE":
        st.error("Please set your API key in the ANTHROPIC_API_KEY environment variable.")
        return
    provider = APIProvider.ANTHROPIC

    # Metadata input section
    st.subheader("Test Metadata")
    col1, col2 = st.columns(2)
    
    with col1:
        test_id = st.text_input("Test ID:", placeholder="e.g., 1, 2, 3...")
    
    with col2:
        categories = {
            "Communication & Forms": "Communication_Forms",
            "Web Automation": "Web_Auto", 
            "Basic App/File Operations": "Basic_App_File",
            "System Monitoring / Data Extraction": "System_Monitor_Data",
            "Programming/Script Tasks": "Programming_Script"
        }
        
        category_display = st.selectbox(
            "Category:",
            options=list(categories.keys()),
            index=0
        )
        category_code = categories[category_display]

    # User input section
    st.subheader("Instruction")
    instruction = st.text_input("Enter your instruction:", "Save an image of a cat to the desktop.")

    if st.button("Run"):
        # Initialize the JSON logger only when Run button is clicked
        timestamp = datetime.datetime.now().strftime('%Y%m%d_%H%M%S')
        test_id_clean = test_id.replace(" ", "_") if test_id else "NoID"
        session_id = f"{test_id_clean}_{category_code}_{timestamp}"
        json_logger = JSONLogger(log_dir=logs_dir, session_id=session_id)
        
        # Initialize cost tracking
        total_input_tokens = 0
        total_output_tokens = 0
        total_cost = 0.0
        
        # Log metadata
        metadata = {
            "Test_ID": test_id if test_id else "Not specified",
            "Cat": category_code
        }
        json_logger.log_event("session_metadata", metadata)
        
        # Log user input only when starting the run
        json_logger.log_user_input(instruction)
        
        st.write(f"Starting Claude 'Computer Use' with instruction: '{instruction}'")

        # Set up the initial messages
        messages: list[BetaMessageParam] = [
            {
                "role": "user",
                "content": instruction,
            }
        ]

        # Define callbacks
        def output_callback(content_block):
            if isinstance(content_block, dict) and content_block.get("type") == "text":
                text = content_block.get("text", "")
                st.write("Assistant:", text)
                json_logger.log_assistant_output(text)
                # Auto-scroll after new content
                st.components.v1.html(auto_scroll_js, height=0)
            else:
                pass  # Non-text content block

        def tool_output_callback(result: ToolResult, tool_use_id: str):
            if result.output:
                output_text = f"> Tool Output [{tool_use_id}]: {result.output}"
                st.write(output_text)
                json_logger.log_tool_result(tool_use_id, output=result.output)
                # Auto-scroll after new content
                st.components.v1.html(auto_scroll_js, height=0)
            if result.error:
                error_text = f"!!! Tool Error [{tool_use_id}]: {result.error}"
                st.write(error_text)
                json_logger.log_tool_result(tool_use_id, error=result.error)
                # Auto-scroll after new content
                st.components.v1.html(auto_scroll_js, height=0)
            if result.base64_image:
                # Save the image to a file if needed
                os.makedirs("screenshots", exist_ok=True)
                image_data = result.base64_image
                image_path = f"screenshots/screenshot_{tool_use_id}.png"
                with open(image_path, "wb") as f:
                    f.write(base64.b64decode(image_data))
                st.image(image_path, caption=f"Screenshot {tool_use_id}")
                json_logger.log_tool_result(tool_use_id, image_path=image_path)
                # Auto-scroll after new content
                st.components.v1.html(auto_scroll_js, height=0)

        def api_response_callback(response: APIResponse[BetaMessage]):
            nonlocal total_input_tokens, total_output_tokens, total_cost
            
            response_content = json.loads(response.text)["content"]  # type: ignore
            response_text = json.dumps(response_content, indent=4)
            st.write(
                "\n---------------\nAPI Response:\n",
                response_text,
                "\n",
            )
            
            # Extract token usage from response if available
            full_response = json.loads(response.text)
            usage = full_response.get("usage", {})
            input_tokens = usage.get("input_tokens", 0)
            output_tokens = usage.get("output_tokens", 0)
            
            if input_tokens > 0 or output_tokens > 0:
                # Update totals
                total_input_tokens += input_tokens
                total_output_tokens += output_tokens
                
                # Calculate cost for this request
                request_cost = calculate_cost(model_name, input_tokens, output_tokens)
                total_cost += request_cost
                
                # Display token usage info
                token_text = (f"**Tokens:** {input_tokens:,} in / {output_tokens:,} out | "
                             f"**Cost:** ${request_cost:.6f}")
                st.info(token_text)
                
                # Log token usage
                token_info = {
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                    "cost_usd": request_cost,
                    "model": model_name
                }
                json_logger.log_event("token_usage", token_info)
            
            json_logger.log_api_response(response_content)
            # Auto-scroll after new content
            st.components.v1.html(auto_scroll_js, height=0)

        # Run the sampling loop
        model_name = "claude-3-5-sonnet-20241022" #"claude-3-5-sonnet-20241022"
        json_logger.log_event("model_selection", {"model": model_name})
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
            
            # Display session summary
            st.success("🎉 **Session Completed Successfully!**")
            
            # Cost summary
            if total_cost > 0:
                st.info(
                    f"💰 **Total Session Cost:** ${total_cost:.6f} USD\n\n"
                    f"📊 **Token Usage:**\n"
                    f"- Input tokens: {total_input_tokens:,}\n"
                    f"- Output tokens: {total_output_tokens:,}\n"
                    f"- Total tokens: {(total_input_tokens + total_output_tokens):,}\n"
                    f"- Model: {model_name}"
                )
                
                # Log final session summary
                session_summary = {
                    "total_input_tokens": total_input_tokens,
                    "total_output_tokens": total_output_tokens,
                    "total_tokens": total_input_tokens + total_output_tokens,
                    "total_cost_usd": total_cost,
                    "model": model_name,
                    "test_id": test_id if test_id else "Not specified",
                    "category": category_code
                }
                json_logger.log_event("session_summary", session_summary)
            
            json_logger.finalize(status="completed")
        except Exception as e:
            error_msg = f"Error during sampling loop: {str(e)}"
            st.error(error_msg)
            json_logger.log_error(error_msg, exception_type=type(e).__name__)
            json_logger.finalize(status="error")
            
    # Simple sidebar for current page
    if os.path.exists(logs_dir):
        json_files = [f for f in os.listdir(logs_dir) if f.endswith('.json')]
        st.sidebar.info(f"📄 {len(json_files)} log files available")
        st.sidebar.write("Switch to 'Analyze Logs' page for detailed analysis.")
    else:
        st.sidebar.info("No logs found yet.")

if __name__ == "__main__":
    main()
