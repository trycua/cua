# Training Computer-Use Models: Creating Human Trajectories with Cua

_Published on May 1, 2025 by Dillon DuPont_

In our previous posts, we covered [building your own Computer-Use Operator](build-your-own-operator-on-macos-1) and [using the Agent framework](build-your-own-operator-on-macos-2) to simplify development. Today, we'll focus on a critical aspect of improving computer-use agents and models: gathering high-quality demonstration data using Cua's Computer-Use Interface (CUI) and its Gradio UI to create and share human-generated trajectories.

Why is this important? Underlying models used by Computer-use agents need examples of how humans interact with computers to learn effectively. By creating a dataset of diverse, well-executed tasks, we can help train better models that understand how to navigate user interfaces and accomplish real tasks.

<video src="https://github.com/user-attachments/assets/c586d460-3877-4b5f-a736-3248886d2134" controls width="600"></video>

## What You'll Learn

By the end of this tutorial, you'll be able to:

- Set up the Computer-Use Interface (CUI) with Gradio UI support
- Record your own computer interaction trajectories
- Organize and tag your demonstrations
- Upload your datasets to Hugging Face for community sharing
- Contribute to improving computer-use AI for everyone

**Prerequisites:**

- macOS Sonoma (14.0) or later
- Python 3.10+
- Basic familiarity with Python and terminal commands
- A Hugging Face account (for uploading datasets)

**Estimated Time:** 20-30 minutes

## Understanding Human Trajectories

### What are Human Trajectories?

Human trajectories, in the context of Computer-use AI Agents, are recordings of how humans interact with computer interfaces to complete tasks. These interactions include:

- Mouse movements, clicks, and scrolls
- Keyboard input
- Changes in the UI state
- Time spent on different elements

These trajectories serve as examples for AI models to learn from, helping them understand the relationship between:

1. The visual state of the screen
2. The user's goal or task
3. The most appropriate action to take

### Why Human Demonstrations Matter

Unlike synthetic data or rule-based automation, human demonstrations capture the nuanced decision-making that happens during computer interaction:

- **Natural Pacing**: Humans pause to think, accelerate through familiar patterns, and adjust to unexpected UI changes
- **Error Recovery**: Humans demonstrate how to recover from mistakes or handle unexpected states
- **Context-Sensitive Actions**: The same UI element might be used differently depending on the task context

By contributing high-quality demonstrations, you're helping to create more capable, human-like computer-use AI systems.

## Setting Up Your Environment

### Installing the CUI with Gradio Support

The Computer-Use Interface includes an optional Gradio UI specifically designed to make recording and sharing demonstrations easy. Let's set it up:

1. **Create a Python environment** (optional but recommended):

   ```bash
   # Using conda
   conda create -n cua-trajectories python=3.10
   conda activate cua-trajectories

   # Using venv
   python -m venv cua-trajectories
   source cua-trajectories/bin/activate  # On macOS/Linux
   ```

2. **Install the CUI package with UI support**:

   ```bash
   pip install "cua-computer[ui]"
   ```

3. **Set up your Hugging Face access token**:
   Create a `.env` file in your project directory and add your Hugging Face token:
   ```bash
   echo "HF_TOKEN=your_huggingface_token" > .env
   ```
   You can get your token from your [Hugging Face account settings](https://huggingface.co/settings/tokens).

### Understanding the Gradio UI

The Computer-Use Interface Gradio UI provides three main components:

1. **Recording Panel**: Captures your screen, mouse, and keyboard activity during demonstrations
2. **Review Panel**: Allows you to review, tag, and organize your demonstration recordings
3. **Upload Panel**: Lets you share your demonstrations with the community via Hugging Face

The UI is designed to make the entire process seamless, from recording to sharing, without requiring deep technical knowledge of the underlying systems.

## Creating Your First Trajectory Dataset

### Launching the UI

To get started, create a simple Python script to launch the Gradio UI:

```python
# launch_trajectory_ui.py
from computer.ui.gradio.app import create_gradio_ui
from dotenv import load_dotenv

# Load your Hugging Face token from .env
load_dotenv('.env')

# Create and launch the UI
app = create_gradio_ui()
app.launch(share=False)
```

Run this script to start the UI:

```bash
python launch_trajectory_ui.py
```

### Recording a Demonstration

Let's walk through the process of recording your first demonstration:

1. **Start the VM**: Click the "Initialize Computer" button in the UI to initialize a fresh macOS sandbox. This ensures your demonstrations are clean and reproducible.
2. **Perform a Task**: Complete a simple task like creating a document, organizing files, or searching for information. Natural, everyday tasks make the best demonstrations.
3. **Review Recording**: Click the "Conversation Logs" or "Function Logs" tabs to review your captured interactions, making sure there is no personal information that you wouldn't want to share.
4. **Add Metadata**: In the "Save/Share Demonstrations" tab, give your recording a descriptive name (e.g., "Creating a Calendar Event") and add relevant tags (e.g., "productivity", "time-management").
5. **Save Your Demonstration**: Click "Save" to store your recording locally.

<video src="https://github.com/user-attachments/assets/de3c3477-62fe-413c-998d-4063e48de176" controls width="600"></video>

### Key Tips for Quality Demonstrations

To create the most valuable demonstrations:

- **Start and end at logical points**: Begin with a clear starting state and end when the task is visibly complete
- **Narrate your thought process**: Use the message input to describe what you're trying to do and why
- **Move at a natural pace**: Don't rush or perform actions artificially slowly
- **Include error recovery**: If you make a mistake, keep going and show how to correct it
- **Demonstrate variations**: Record multiple ways to complete the same task

## Organizing and Tagging Demonstrations

Effective tagging and organization make your demonstrations more valuable to researchers and model developers. Consider these tagging strategies:

### Task-Based Tags

Describe what the demonstration accomplishes:

- `web-browsing`
- `document-editing`
- `file-management`
- `email`
- `scheduling`

### Application Tags

Identify the applications used:

- `finder`
- `safari`
- `notes`
- `terminal`
- `calendar`

### Complexity Tags

Indicate the difficulty level:

- `beginner`
- `intermediate`
- `advanced`
- `multi-application`

### UI Element Tags

Highlight specific UI interactions:

- `drag-and-drop`
- `menu-navigation`
- `form-filling`
- `search`

The Computer-Use Interface UI allows you to apply and manage these tags across all your saved demonstrations, making it easy to create cohesive, well-organized datasets.

<video src="https://github.com/user-attachments/assets/5ad1df37-026a-457f-8b49-922ae805faef" controls width="600"></video>

## Uploading to Hugging Face

Sharing your demonstrations helps advance research in computer-use AI. The Gradio UI makes uploading to Hugging Face simple:

### Preparing for Upload

1. **Review Your Demonstrations**: Use the review panel to ensure all demonstrations are complete and correctly tagged.

2. **Select Demonstrations to Upload**: You can upload all demonstrations or filter by specific tags.

3. **Configure Dataset Information**:
   - **Repository Name**: Format as `{your_username}/{dataset_name}`, e.g., `johndoe/productivity-tasks`
   - **Visibility**: Choose `public` to contribute to the community or `private` for personal use
   - **License**: Standard licenses like CC-BY or MIT are recommended for public datasets

### The Upload Process

1. **Click "Upload to Hugging Face"**: This initiates the upload preparation.

2. **Review Dataset Summary**: Confirm the number of demonstrations and total size.

3. **Confirm Upload**: The UI will show progress as files are transferred.

4. **Receive Confirmation**: Once complete, you'll see a link to your new dataset on Hugging Face.

<video src="https://github.com/user-attachments/assets/c586d460-3877-4b5f-a736-3248886d2134" controls width="600"></video>

Your uploaded dataset will have a standardized format with the following structure:

```json
{
  "timestamp": "2025-05-01T09:20:40.594878",
  "session_id": "1fe9f0fe-9331-4078-aacd-ec7ffb483b86",
  "name": "penguin lemon forest",
  "tool_calls": [...],  // Detailed interaction records
  "messages": [...],    // User/assistant messages
  "tags": ["highquality", "tasks"],
  "images": [...]       // Screenshots of each state
}
```

This structured format makes it easy for researchers to analyze patterns across different demonstrations and build better computer-use models.

```python
from computer import Computer

computer = Computer(os_type="macos", display="1024x768", memory="8GB", cpu="4")
try:
    await computer.run()

    screenshot = await computer.interface.screenshot()
    with open("screenshot.png", "wb") as f:
        f.write(screenshot)

    await computer.interface.move_cursor(100, 100)
    await computer.interface.left_click()
    await computer.interface.right_click(300, 300)
    await computer.interface.double_click(400, 400)

    await computer.interface.type("Hello, World!")
    await computer.interface.press_key("enter")

    await computer.interface.set_clipboard("Test clipboard")
    content = await computer.interface.copy_to_clipboard()
    print(f"Clipboard content: {content}")
finally:
    await computer.stop()
```

## Example: Shopping List Demonstration

Let's walk through a concrete example of creating a valuable demonstration:

### Task: Adding Shopping List Items to a Doordash Cart

1. **Start Recording**: Begin with a clean desktop and a text file containing a shopping list.

2. **Task Execution**: Open the file, read the list, open Safari, navigate to Doordash, and add each item to the cart.

3. **Narration**: Add messages like "Reading the shopping list" and "Searching for rice on Doordash" to provide context.

4. **Completion**: Verify all items are in the cart and end the recording.

5. **Tagging**: Add tags like `shopping`, `web-browsing`, `task-completion`, and `multi-step`.

This type of demonstration is particularly valuable because it showcases real-world task completion requiring multiple applications and context switching.

### Exploring Community Datasets

You can also learn from existing trajectory datasets contributed by the community:

1. Visit [Hugging Face Datasets tagged with 'cua'](https://huggingface.co/datasets?other=cua)
2. Explore different approaches to similar tasks
3. Download and analyze high-quality demonstrations

## Conclusion

### Summary

In this guide, we've covered how to:

- Set up the Computer-Use Interface with Gradio UI
- Record high-quality human demonstrations
- Organize and tag your trajectories
- Share your datasets with the community

By contributing your own demonstrations, you're helping to build more capable, human-like AI systems that can understand and execute complex computer tasks.

### Next Steps

Now that you know how to create and share trajectories, consider these advanced techniques:

- Create themed collections around specific productivity workflows
- Collaborate with others to build comprehensive datasets
- Use your datasets to fine-tune your own computer-use models

### Resources

- [Computer-Use Interface GitHub](https://github.com/trycua/cua/tree/main/libs/computer)
- [Hugging Face Datasets Documentation](https://huggingface.co/docs/datasets)
- [Example Dataset: ddupont/test-dataset](https://huggingface.co/datasets/ddupont/test-dataset)
