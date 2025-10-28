# Pluggable AI Model Agent Loop Testing Framework

A testing framework that allows you to **plug in any AI model** to test it with the agent loop. The agent loop interacts with a **minimal mock computer** that only provides screenshot functionality and returns a static image.

## 🎯 Purpose

This framework tests **any AI model** with a **minimal agent loop**:

1. **Plug in AI Model** - Use any AI model (Anthropic, OpenAI, custom, or test model)
2. **Minimal Mock Computer** - Only provides screenshot functionality (no complex computer actions)
3. **Agent Loop** - Coordinates between AI model and mock computer
4. **Static Image** - Uses a static PNG image as the "VM" for consistent testing
5. **Verify Loop Works** - Confirm the agent loop executes without crashing

## 📁 Structure

```
tests/agent_loop_testing/
├── __init__.py                    # Package initialization
├── README.md                      # This file
├── agent_test.py                  # Main test runner with pluggable AI models
├── ai_interface.py                # AI model interfaces and agent loop
├── mock_computer.py               # Minimal mock computer (screenshot only)
└── test_images/
    └── image.png                   # Static macOS desktop image
```

## 🚀 Quick Start

### Run with Different AI Models

```bash
# Test with a simple test model (no external dependencies)
python tests/agent_loop_testing/agent_test.py --model test-model

# Test with Anthropic Claude
python tests/agent_loop_testing/agent_test.py --model anthropic/claude-sonnet-4-20250514

# Test with OpenAI GPT-4o Mini
python tests/agent_loop_testing/agent_test.py --model openai/gpt-4o-mini

# Test with custom image
python tests/agent_loop_testing/agent_test.py --model test-model --image /path/to/image.png

# Test with custom parameters
python tests/agent_loop_testing/agent_test.py --model test-model --max-iterations 3 --message "Click on Safari"
```

### Install Dependencies (for external AI models)

```bash
# For CUA models (Anthropic, OpenAI, etc.)
pip install -e libs/python/agent -e libs/python/computer

# For test model only (no additional dependencies needed)
# Just run the test directly
```

## 🧪 What This Tests

### ✅ **PASS Criteria**

- AI model initializes successfully
- AI model can analyze screenshots
- AI model generates logical responses
- Agent loop executes multiple iterations
- Agent loop doesn't crash or break
- Screenshot functionality works correctly

### ❌ **FAIL Criteria**

- AI model fails to initialize
- AI model crashes during execution
- Agent loop breaks unexpectedly
- Screenshot functionality fails
- External API errors (for cloud models)

## 🔧 Architecture

### Pluggable AI Models

The framework supports multiple types of AI models:

1. **Test Model** (`test-model`): Simple deterministic model for testing
2. **CUA Models**: Anthropic, OpenAI, and other models via CUA's ComputerAgent
3. **Custom Models**: Implement `AIModelInterface` for custom models

### Minimal Mock Computer

The mock computer only provides:

- `screenshot()`: Returns static image as base64
- `get_screen_dimensions()`: Returns screen size
- Action counting and statistics

**No complex computer actions** - just screenshot functionality for testing AI model capabilities.

### Agent Loop

The `AgentLoop` class coordinates:

- Taking screenshots from mock computer
- Sending messages + images to AI model
- Processing AI model responses
- Managing conversation history
- Iteration control and limits

## 📊 Example Output

```
🤖 Testing Agent Loop with AI Model: test-model
================================================================================
✅ Step 1: Creating AI model: test-model
✅ Step 2: Creating mock computer with static image
✅ Step 3: Creating agent loop
✅ Step 4: Starting agent execution...

================================================================================
AGENT EXECUTION:
================================================================================

--- Iteration 1 ---
🔄 AI Response: I can see a macOS desktop with Safari, Terminal, and Finder icons in the dock. Let me click on Safari to open it.
🔧 Tool Call: click with args: {'x': 125, 'y': 975}
📊 Screenshots taken: True
📊 Conversation length: 2

--- Iteration 2 ---
🔄 AI Response: I clicked on Safari but it didn't open. Let me try clicking on Terminal instead.
🔧 Tool Call: click with args: {'x': 225, 'y': 975}
📊 Screenshots taken: True
📊 Conversation length: 4

--- Iteration 3 ---
🔄 AI Response: I've tried clicking on different icons but nothing seems to be happening. This might be a static image. Let me finish here.
🔧 No tool calls made
📊 Screenshots taken: True
📊 Conversation length: 6
🏁 Agent finished

================================================================================
AGENT EXECUTION COMPLETE
================================================================================
✅ Total iterations: 3
✅ Total screenshots: 3
✅ Model: test-model
✅ Screen dimensions: 1920x1080

================================================================================
🎉 AGENT LOOP TEST COMPLETE!
================================================================================

This proves:
• AI model 'test-model' works with the agent loop
• Mock computer serves static image successfully
• Agent loop executes multiple iterations without crashing
• AI model can analyze screenshots and generate responses
• Tool calling interface works correctly
```

## 🔌 Adding Custom AI Models

To add a custom AI model, implement the `AIModelInterface`:

```python
from .ai_interface import AIModelInterface, AgentResponse, AgentMessage, MockComputerInterface

class MyCustomAIModel(AIModelInterface):
    def __init__(self, model_name: str):
        self.model_name = model_name

    async def generate_response(
        self,
        messages: List[AgentMessage],
        computer_interface: MockComputerInterface
    ) -> AgentResponse:
        # Your AI model logic here
        # Take screenshot: await computer_interface.take_screenshot()
        # Process messages and generate response
        return AgentResponse(
            content="Your response here",
            tool_calls=[{"name": "click", "args": {"x": 100, "y": 200}}],
            finished=False
        )

    def get_model_name(self) -> str:
        return self.model_name
```

Then use it:

```python
from .agent_test import create_ai_model
ai_model = create_ai_model("my-custom-model")
```

## ⚙️ Configuration Options

### Command Line Arguments

```bash
python agent_test.py [options]

Options:
  --model MODEL           AI model to test (default: test-model)
  --image PATH            Path to static image file (optional)
  --max-iterations N      Maximum iterations (default: 5)
  --message TEXT          Initial message to agent
  -h, --help              Show help message
```

### Model Names

- `test-model`: Simple test model (no external dependencies)
- `anthropic/claude-sonnet-4-20250514`: Anthropic Claude via CUA
- `openai/gpt-4o-mini`: OpenAI GPT-4o Mini via CUA
- `custom-model-name`: Any custom model name

### Image Sources

1. **Provided image**: `--image /path/to/image.png`
2. **Default image**: `test_images/image.png` (if exists)
3. **Generated image**: Creates default macOS desktop if no image found

## 🚀 GitHub Actions Integration

The framework works in CI/CD environments:

```yaml
- name: Test AI models with agent loop
  run: |
    cd tests/agent_loop_testing
    python agent_test.py --model test-model
    python agent_test.py --model anthropic/claude-sonnet-4-20250514
```

## 🎉 Key Benefits

### ✅ **Pluggable Architecture**

- Test any AI model with the same interface
- Easy to add new models
- Consistent testing across different providers

### ✅ **Minimal Dependencies**

- Test model works without external APIs
- Mock computer only implements what's needed
- Clean separation of concerns

### ✅ **Focused Testing**

- Tests AI model capabilities, not computer functionality
- Verifies agent loop mechanics
- Consistent results with static images

### ✅ **Easy to Use**

- Simple command-line interface
- Clear output and error messages
- Works in CI/CD environments

**Perfect for testing AI models with agent loops without the complexity of full computer implementations!**
