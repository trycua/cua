# Bringing Your Own Agent to Windows Agent Arena

Want to test your own agents in Windows Agent Arena? You can use our default agent as a template and create your own folder under `src/win-arena-container/client/mm_agents`. You just need to ensure that your `agent.py` file includes the `predict()` and `reset()` functions.

## Steps to Create Your Custom Agent

### 1. Create a New Agent Folder

Navigate to the `mm_agents` directory:

```bash
cd src/win-arena-container/client/mm_agents
```

Create a new folder for your agent. For example, if your agent is named `my_agent`, run:

```bash
mkdir my_agent
```

### 2. Use the Default Agent as a Template

Copy the default agent's `agent.py` into your new folder:

```bash
cp default_agent/agent.py my_agent/
```

### 3. Implement Required Functions

In your `agent.py`, you must implement the following functions:

- **`predict(instruction: str, obs: Dict) -> List`**: Generates a list of actions based on the given instruction and observation.
- **`reset()`**: Resets the agent's internal state.

### 4. Example Implementation

Below is an example implementation of our agent called **Navi**:

```python
# agent.py
import logging
from typing import Dict, List
from PIL import Image
from io import BytesIO
import copy

logger = logging.getLogger("desktopenv.agent")

class NaviAgent:
    def __init__(
            self,
            server: str = "azure",
            model: str = "gpt-4o",
            som_config = None,
            som_origin = "oss",
            obs_view = "screen",
            auto_window_maximize = False,
            use_last_screen = True,
            temperature: float = 0.5,
    ):
        # Initialize agent parameters
        self.action_space = "code_block"
        self.server = server
        self.model = model
        # ... (additional initialization)

    def predict(self, instruction: str, obs: Dict) -> List:
        """
        Predict the next action(s) based on the current observation.
        """
        # Process the observation
        # Generate actions based on the instruction
        # ...
        actions = ["# Your code logic here"]
        return actions

    def reset(self):
        """
        Reset the agent's internal state.
        """
        # Reset logic
        pass
```

### 5. Customize Your Agent Logic

Modify the `predict()` and `reset()` methods to implement your agent's specific behavior. Utilize any necessary libraries or models to enhance functionality.

### 6. Test Your Agent

Ensure your agent works correctly by testing it within the environment:

```python
from mm_agents.my_agent.agent import MyAgent

agent = MyAgent()
obs = get_current_observation()  # Function to retrieve the current observation
instruction = "Your test instruction here"
actions = agent.predict(instruction, obs)
execute_actions(actions)  # Function to execute the predicted actions
```

### 7. Submit a Pull Request

Once your agent is ready, submit a Pull Request (PR) to the repository with your new agent folder and code changes. Ensure your code follows the project's guidelines and is well-documented.

## Important Considerations

- **Observation Data**: The `obs` dictionary contains vital information like screenshots, window titles, and clipboard content. Use this data to inform your agent's decisions.
- **Action Format**: The list returned by `predict()` should contain executable actions or code blocks that the environment can interpret.
- **Logging**: Use the `logging` module to help debug your agent. This can be invaluable for tracking down issues.