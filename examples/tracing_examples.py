"""
Examples demonstrating the Computer.tracing API for recording sessions.

This module shows various use cases for the new Computer.tracing functionality,
including training data collection, debugging, and compliance recording.
"""

import asyncio
import logging
from pathlib import Path
from computer import Computer
from agent import ComputerAgent


async def basic_tracing_example():
    """
    Basic example showing how to use Computer.tracing for recording a simple session.
    """
    print("=== Basic Tracing Example ===")
    
    # Initialize computer
    computer = Computer(os_type="macos", provider_type="lume")
    await computer.run()
    
    try:
        # Start tracing with basic configuration
        await computer.tracing.start({
            'screenshots': True,
            'api_calls': True,
            'metadata': True,
            'name': 'basic_session'
        })
        
        print("Tracing started...")
        
        # Perform some computer operations
        await computer.interface.move_cursor(100, 100)
        await computer.interface.left_click()
        await computer.interface.type_text("Hello, tracing!")
        await computer.interface.press_key("enter")
        
        # Add custom metadata
        await computer.tracing.add_metadata("session_type", "basic_demo")
        await computer.tracing.add_metadata("user_notes", "Testing basic functionality")
        
        # Stop tracing and save
        trace_path = await computer.tracing.stop({'format': 'zip'})
        print(f"Trace saved to: {trace_path}")
        
    finally:
        await computer.stop()


async def agent_tracing_example():
    """
    Example showing how to use tracing with ComputerAgent for enhanced session recording.
    """
    print("=== Agent with Tracing Example ===")
    
    # Initialize computer and agent
    computer = Computer(os_type="macos", provider_type="lume")
    await computer.run()
    
    try:
        # Start comprehensive tracing
        await computer.tracing.start({
            'screenshots': True,
            'api_calls': True,
            'accessibility_tree': True,  # Include accessibility data for training
            'metadata': True,
            'name': 'agent_session'
        })
        
        # Create agent
        agent = ComputerAgent(
            model="openai/computer-use-preview",
            tools=[computer],
            verbosity=logging.INFO
        )
        
        # Add metadata about the agent session
        await computer.tracing.add_metadata("agent_model", "openai/computer-use-preview")
        await computer.tracing.add_metadata("task_type", "web_search")
        
        # Run agent task
        async for message in agent.run("Open a web browser and search for 'computer use automation'"):
            print(f"Agent: {message}")
            
        # Stop tracing
        trace_path = await computer.tracing.stop({'format': 'zip'})
        print(f"Agent trace saved to: {trace_path}")
        
    finally:
        await computer.stop()


async def custom_agent_tracing_example():
    """
    Example showing tracing with custom agent implementations.
    """
    print("=== Custom Agent Tracing Example ===")
    
    computer = Computer(os_type="macos", provider_type="lume")
    await computer.run()
    
    try:
        # Start tracing with custom path
        trace_dir = Path.cwd() / "custom_traces" / "my_agent_session"
        await computer.tracing.start({
            'screenshots': True,
            'api_calls': True,
            'accessibility_tree': False,
            'metadata': True,
            'path': str(trace_dir)
        })
        
        # Custom agent logic using direct computer calls
        await computer.tracing.add_metadata("session_type", "custom_agent")
        await computer.tracing.add_metadata("purpose", "RPA_workflow")
        
        # Take initial screenshot
        screenshot = await computer.interface.screenshot()
        
        # Simulate RPA workflow
        await computer.interface.move_cursor(500, 300)
        await computer.interface.left_click()
        await computer.interface.type_text("automation workflow test")
        
        # Add workflow checkpoint
        await computer.tracing.add_metadata("checkpoint", "text_input_complete")
        
        await computer.interface.hotkey("command", "a")  # Select all
        await computer.interface.hotkey("command", "c")  # Copy
        
        # Stop tracing and save as directory
        trace_path = await computer.tracing.stop({'format': 'dir'})
        print(f"Custom agent trace saved to: {trace_path}")
        
    finally:
        await computer.stop()


async def training_data_collection_example():
    """
    Example for collecting training data with rich context.
    """
    print("=== Training Data Collection Example ===")
    
    computer = Computer(os_type="macos", provider_type="lume")
    await computer.run()
    
    try:
        # Start tracing optimized for training data
        await computer.tracing.start({
            'screenshots': True,        # Essential for visual training
            'api_calls': True,         # Capture action sequences
            'accessibility_tree': True, # Rich semantic context
            'metadata': True,          # Custom annotations
            'name': 'training_session'
        })
        
        # Add training metadata
        await computer.tracing.add_metadata("data_type", "training")
        await computer.tracing.add_metadata("task_category", "ui_automation")
        await computer.tracing.add_metadata("difficulty", "intermediate")
        await computer.tracing.add_metadata("annotator", "human_expert")
        
        # Simulate human demonstration
        await computer.interface.screenshot()  # Baseline screenshot
        
        # Step 1: Navigate to application
        await computer.tracing.add_metadata("step", "1_navigate_to_app")
        await computer.interface.move_cursor(100, 50)
        await computer.interface.left_click()
        
        # Step 2: Input data
        await computer.tracing.add_metadata("step", "2_input_data")
        await computer.interface.type_text("training example data")
        
        # Step 3: Process
        await computer.tracing.add_metadata("step", "3_process")
        await computer.interface.press_key("tab")
        await computer.interface.press_key("enter")
        
        # Final metadata
        await computer.tracing.add_metadata("success", True)
        await computer.tracing.add_metadata("completion_time", "45_seconds")
        
        trace_path = await computer.tracing.stop()
        print(f"Training data collected: {trace_path}")
        
    finally:
        await computer.stop()


async def debugging_session_example():
    """
    Example for debugging agent behavior with detailed tracing.
    """
    print("=== Debugging Session Example ===")
    
    computer = Computer(os_type="macos", provider_type="lume")
    await computer.run()
    
    try:
        # Start tracing for debugging
        await computer.tracing.start({
            'screenshots': True,
            'api_calls': True,
            'accessibility_tree': True,
            'metadata': True,
            'name': 'debug_session'
        })
        
        # Debug metadata
        await computer.tracing.add_metadata("session_type", "debugging")
        await computer.tracing.add_metadata("issue", "click_target_detection")
        await computer.tracing.add_metadata("expected_behavior", "click_on_button")
        
        try:
            # Problematic sequence that needs debugging
            await computer.interface.move_cursor(200, 150)
            await computer.interface.left_click()
            
            # This might fail - let's trace it
            await computer.interface.type_text("debug test")
            await computer.tracing.add_metadata("action_result", "successful_typing")
            
        except Exception as e:
            # Record the error in tracing
            await computer.tracing.add_metadata("error_encountered", str(e))
            await computer.tracing.add_metadata("error_type", type(e).__name__)
            print(f"Error occurred: {e}")
            
        # Stop tracing
        trace_path = await computer.tracing.stop()
        print(f"Debug trace saved: {trace_path}")
        print("Use this trace to analyze the failure and improve the agent")
        
    finally:
        await computer.stop()


async def human_in_the_loop_example():
    """
    Example for recording mixed human/agent sessions.
    """
    print("=== Human-in-the-Loop Example ===")
    
    computer = Computer(os_type="macos", provider_type="lume")
    await computer.run()
    
    try:
        # Start tracing for hybrid session
        await computer.tracing.start({
            'screenshots': True,
            'api_calls': True,
            'metadata': True,
            'name': 'human_agent_collaboration'
        })
        
        # Initial agent phase
        await computer.tracing.add_metadata("phase", "agent_autonomous")
        await computer.tracing.add_metadata("agent_model", "computer-use-preview")
        
        # Agent performs initial task
        await computer.interface.move_cursor(300, 200)
        await computer.interface.left_click()
        await computer.interface.type_text("automated input")
        
        # Transition to human intervention
        await computer.tracing.add_metadata("phase", "human_intervention")
        await computer.tracing.add_metadata("intervention_reason", "complex_ui_element")
        
        print("Human intervention phase - manual actions will be recorded...")
        # At this point, human can take control while tracing continues
        
        # Simulate human input (in practice, this would be actual human interaction)
        await computer.interface.move_cursor(500, 400)
        await computer.interface.double_click()
        await computer.tracing.add_metadata("human_action", "double_click_complex_element")
        
        # Back to agent
        await computer.tracing.add_metadata("phase", "agent_completion")
        await computer.interface.press_key("enter")
        
        trace_path = await computer.tracing.stop()
        print(f"Human-agent collaboration trace saved: {trace_path}")
        
    finally:
        await computer.stop()


async def performance_monitoring_example():
    """
    Example for performance monitoring and analysis.
    """
    print("=== Performance Monitoring Example ===")
    
    computer = Computer(os_type="macos", provider_type="lume")
    await computer.run()
    
    try:
        # Start tracing for performance analysis
        await computer.tracing.start({
            'screenshots': False,  # Skip screenshots for performance
            'api_calls': True,
            'metadata': True,
            'name': 'performance_test'
        })
        
        # Performance test metadata
        await computer.tracing.add_metadata("test_type", "performance_benchmark")
        await computer.tracing.add_metadata("expected_duration", "< 30 seconds")
        
        import time
        start_time = time.time()
        
        # Perform a series of rapid actions
        for i in range(10):
            await computer.tracing.add_metadata("iteration", i)
            await computer.interface.move_cursor(100 + i * 50, 100)
            await computer.interface.left_click()
            await computer.interface.type_text(f"Test {i}")
            await computer.interface.press_key("tab")
            
        end_time = time.time()
        
        # Record performance metrics
        await computer.tracing.add_metadata("actual_duration", f"{end_time - start_time:.2f} seconds")
        await computer.tracing.add_metadata("actions_per_second", f"{40 / (end_time - start_time):.2f}")
        
        trace_path = await computer.tracing.stop()
        print(f"Performance trace saved: {trace_path}")
        
    finally:
        await computer.stop()


async def main():
    """
    Run all tracing examples.
    """
    print("Computer.tracing API Examples")
    print("=" * 50)
    
    examples = [
        basic_tracing_example,
        agent_tracing_example,
        custom_agent_tracing_example,
        training_data_collection_example,
        debugging_session_example,
        human_in_the_loop_example,
        performance_monitoring_example
    ]
    
    for example in examples:
        try:
            await example()
            print()
        except Exception as e:
            print(f"Error in {example.__name__}: {e}")
            print()
            
    print("All examples completed!")


if __name__ == "__main__":
    asyncio.run(main())