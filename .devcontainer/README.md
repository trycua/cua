# Dev Container Setup

This repository includes a Dev Container configuration that simplifies the development setup to just 3 steps:

## Quick Start

![Clipboard-20250611-180809-459](https://github.com/user-attachments/assets/447eaeeb-0eec-4354-9a82-44446e202e06)

1. **Install the Dev Containers extension ([VS Code](https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.remote-containers) or [WindSurf](https://docs.windsurf.com/windsurf/advanced#dev-containers-beta))**
2. **Open the repository in the Dev Container:**
    - Press `Ctrl+Shift+P` (or `⌘+Shift+P` on macOS)
    - Select `Dev Containers: Clone Repository in Container Volume...` and paste the repository URL: `https://github.com/trycua/cua.git` (if not cloned) or `Dev Containers: Open Folder in Container...` (if git cloned).
     > **Note**: On WindSurf, the post install hook might not run automatically. If so, run `/bin/bash .devcontainer/post-install.sh` manually.
3. **Open the VS Code workspace:** Once the post-install.sh is done running, open the `.vscode/py.code-workspace` workspace and press ![Open Workspace](https://github.com/user-attachments/assets/923bdd43-8c8f-4060-8d78-75bfa302b48c)
.
4. **Run the Agent UI example:** Click ![Run Agent UI](https://github.com/user-attachments/assets/7a61ef34-4b22-4dab-9864-f86bf83e290b)
 to start the Gradio UI. If prompted to install **debugpy (Python Debugger)** to enable remote debugging, select 'Yes' to proceed.
5. **Access the Gradio UI:** The Gradio UI will be available at `http://localhost:7860` and will automatically forward to your host machine.

## What's Included

The dev container automatically:

- ✅ Sets up Python 3.11 environment
- ✅ Installs all system dependencies (build tools, OpenGL, etc.)
- ✅ Configures Python paths for all packages
- ✅ Installs Python extensions (Black, Ruff, Pylance)
- ✅ Forwards port 7860 for the Gradio web UI
- ✅ Mounts your source code for live editing
- ✅ Creates the required `.env.local` file

## Running Examples

After the container is built, you can run examples directly:

```bash
# Run the agent UI (Gradio web interface)
python examples/agent_ui_examples.py

# Run computer examples
python examples/computer_examples.py

# Run computer UI examples
python examples/computer_ui_examples.py
```

The Gradio UI will be available at `http://localhost:7860` and will automatically forward to your host machine.

## Environment Variables

You'll need to add your API keys to `.env.local`:

```bash
# Required for Anthropic provider
ANTHROPIC_API_KEY=your_anthropic_key_here

# Required for OpenAI provider
OPENAI_API_KEY=your_openai_key_here
```

## Notes

- The container connects to `host.docker.internal:7777` for Lume server communication
- All Python packages are pre-installed and configured
- Source code changes are reflected immediately (no rebuild needed)
- The container uses the same Dockerfile as the regular Docker development environment
