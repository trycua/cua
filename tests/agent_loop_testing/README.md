# CUA Agent Test

Simple test for CUA ComputerAgent SDK with mock computer.

## Run Test

```bash
python tests/agent_loop_testing/agent_test.py --model anthropic/claude-sonnet-4-20250514
```

## What It Does

- Tests real CUA ComputerAgent SDK
- Uses mock computer (only screenshots, no real actions)
- Agent tries to "Open Safari browser" 
- Runs up to 5 iterations
- Shows agent responses and tool calls

## What Passes ✅

- Agent initializes
- Takes screenshots
- Analyzes images
- Makes tool calls
- Runs multiple iterations

## What Fails ❌

- Missing dependencies
- Invalid API keys
- Agent crashes
- Import errors

## Install

```bash
pip install -e libs/python/agent -e libs/python/computer
export ANTHROPIC_API_KEY="your-key"
```

## Example Output

```
🤖 Testing CUA Agent: anthropic/claude-sonnet-4-20250514
==================================================
✅ CUA Agent created
✅ Mock computer ready
🚀 Running agent...

Iteration 1:
  Agent: I'll click on Safari to open it.
  Tool: click {'x': 125, 'y': 975}

Iteration 2:
  Agent: Safari didn't open, let me try again.
  Tool: click {'x': 125, 'y': 975}

Iteration 3:
  Agent: This appears to be a static test environment.

🏁 Stopping after 5 iterations (safety limit)

==================================================
🎉 TEST COMPLETE!
==================================================
✅ Model: anthropic/claude-sonnet-4-20250514
✅ Iterations: 3
✅ Screenshots: 3
✅ Agent executed successfully
```