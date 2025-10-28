# Agent Loop Testing Framework

A testing framework that spins up an **agent** and uses a static screenshot as the "VM". The agent will repeatedly try to click on Safari, but since it's just a static image, it will keep trying. We verify the agent loop doesn't break.

## 🎯 Purpose

This framework tests a **ComputerAgent** with a **static screenshot** as the "VM":

1. **Spin up Agent** - Initialize ComputerAgent with LLM
2. **Use Mock Computer** - Use a mock computer that serves your static PNG image
3. **Give Agent Task** - Ask it to "Take a screenshot and tell me what you see"
4. **Monitor Agent Loop** - Watch it execute the task
5. **Verify Loop Works** - Confirm the agent completes without crashing

## 📁 Structure

```
tests/agent_loop_testing/
├── __init__.py                    # Package initialization
├── README.md                      # This file
├── real_agent_test.py             # Real agent test with static screenshot
└── test_images/
    └── image.png                   # Your macOS desktop image
```

## 🚀 Quick Start

### Run the Agent Test

```bash
# Install dependencies first
pip install -e libs/python/agent -e libs/python/computer

# Run the agent test (no computer server needed!)
python tests/agent_loop_testing/agent_test.py
```

### Expected Output

```
🤖 Testing Agent Loop with Static Screenshot
============================================================
✅ Step 1: Created computer handler with static PNG
✅ Step 2: Created ComputerAgent
✅ Step 3: Starting agent execution...

============================================================
AGENT EXECUTION:
============================================================

--- Iteration 1 ---
🔄 Agent response: I can see a macOS desktop with Safari in the dock...
🔧 Tool call: click
✅ Tool result: completed

--- Iteration 2 ---
🔄 Agent response: I clicked on Safari but nothing happened. Let me try again...
🔧 Tool call: click
✅ Tool result: completed

--- Iteration 3 ---
🔄 Agent response: The Safari icon still hasn't responded. Let me try a different approach...
🔧 Tool call: double_click
✅ Tool result: completed

🛑 Stopping after 3 iterations to test loop mechanics

============================================================
AGENT EXECUTION COMPLETE
============================================================
✅ Agent completed successfully

============================================================
🎉 AGENT LOOP TEST COMPLETE!
============================================================

This proves:
• Mock computer serves your static PNG image
• ComputerAgent works with mock computer
• Agent loop executes multiple iterations without crashing
• Agent can take screenshots, analyze, and make tool calls repeatedly
• LLM and provider are working correctly
• Agent loop mechanics are robust
```

## 🧪 What This Tests

### ✅ **PASS Criteria**
- Agent initializes successfully
- Agent takes screenshots from static image
- Agent analyzes the image and generates logical actions
- Agent executes actions (clicks, types, etc.)
- Agent loop continues even when actions have no effect
- Agent doesn't crash or break the loop
- LLM and provider are working correctly

### ❌ **FAIL Criteria**
- Agent fails to initialize
- Agent crashes during execution
- Agent loop breaks or stops unexpectedly
- LLM fails to analyze images or generate actions
- Actions fail to execute

## ⚠️ Important Notes

**This tests the agent loop mechanics, not agent correctness.**

**Prerequisites:**
- Valid API keys for the LLM provider (Anthropic, OpenAI, etc.)
- Dependencies installed (`pip install -e libs/python/agent -e libs/python/computer`)
- Your static PNG image in `test_images/image.png`

The workflow:
1. Agent sees static screenshot (thinks it's a real VM)
2. Agent tries to click on Safari icon
3. Agent takes another screenshot (gets same static image)
4. Agent realizes nothing changed and tries again
5. Agent tries different approaches (double-click, different coordinates)
6. This repeats for 3 iterations (proving loop doesn't break)

This tests:
- ✅ Agent initialization and execution
- ✅ Screenshot analysis and action generation
- ✅ Action execution and loop mechanics
- ✅ Error handling when actions have no effect
- ✅ LLM connectivity and functionality

This does NOT test:
- ❌ Agent correctness (whether it clicks the right things)
- ❌ Real-world behavior with actual UI changes
- ❌ Complex scenarios or edge cases

## 🔧 Configuration

### Timeout Settings

The test has built-in safety limits:

```python
# In real_agent_test.py
agent = ComputerAgent(
    model="gpt-4o-mini",  # Lightweight model for testing
    max_iterations=5,     # Limit iterations
)

# Test timeout
result = await asyncio.wait_for(
    agent.run(task),
    timeout=60.0  # 60 second timeout
)
```

### Custom Screenshot

The test uses your provided screenshot:

```python
screenshot_path = Path(__file__).parent / "test_images" / "image.png"
```

If the file doesn't exist, it creates a default macOS desktop with Safari icon.

## 🚀 GitHub Actions Integration

The framework includes a GitHub Actions workflow that runs:

```yaml
- name: Run agent loop test
  run: |
    cd tests/agent_loop_testing
    python agent_test.py
```

## 🎉 Ready for Use

This agent testing framework is:

- ✅ **Agent**: Uses actual ComputerAgent with LLM
- ✅ **Static VM**: Uses your screenshot as the "VM"
- ✅ **Loop Testing**: Verifies agent loop doesn't break
- ✅ **Timeout Protected**: Won't run forever
- ✅ **CI/CD Ready**: Works in GitHub Actions
- ✅ **Focused**: Tests only loop mechanics, not correctness

**Perfect for verifying that your agent and LLM provider work correctly!**