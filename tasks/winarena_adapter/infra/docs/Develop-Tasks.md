# Develop new Tasks in WAA

First and foremost, we recommend getting familiar with our own task examples to get a sense of the configuration required for setting up a task as well as common setups, commands, patterns, etc. We also provide a brief example below. If developing a task for a program/application category that already exists in this repo (e.g., Chrome, Edge, LibreOffice, etc.), we highly recommend getting familiar with our existing task machineary including but not limited to its getters, evaluators, etc. and making modifications from there.

## Task Definition & Configuration  

### Task JSON Configuration Example  
  
```json  
{  
    "id": "8ba5ae7a-5ae5-4eab-9fcc-5dd4fe3abf89-W0S",  
    "instruction": "Help me modify the folder used to store my recordings to the Desktop",  
    "config": [  
        {  
            "type": "launch",  
            "parameters": {  
                "command": "vlc"  
            }  
        },  
        {  
            "type": "execute",  
            "parameters": {  
                "command": [  
                    "python",  
                    "-c",  
                    "import pyautogui; import time; pyautogui.click(960, 540); time.sleep(0.5);"  
                ]  
            }  
        }  
    ],  
    "evaluator": {  
        "func": "vis_vlc_recordings_folder",  
        "expected": {  
            "type": "rule",  
            "rules": {  
                "recording_file_path": "C:\\Users\\Docker\\Desktop"  
            }  
        }  
    },  
    "result": {  
        "type": "vlc_config",  
        "dest": "vlcrc"  
    }  
}
```
 
Figure above: Task JSON configuration example defined by five key components represented by the colored JSON keys: a task ID, an instruction, an initial configuration (config), an evaluator, and a result.

To better illustrate how tasks are defined, configured, and implemented, we refer to an example of a task JSON shown above. Each task follows a similar format:

- ID (id): A unique identifier for each task.
- Instruction (instruction): Defines the task in natural language. In this example, the task is: "Help me modify the folder used to store my recordings to the Desktop".
- Configuration (config): Sets up the initial state for the agent. It can include actions such as launching programs, executing commands, or downloading files. In this case:
  1. It launches the VLC player.
  2. Simulates a mouse click using pyautogui after launching VLC.
- Evaluator (evaluator): Defines how the task is evaluated. It specifies the function (vis_vlc_recordings_folder) and the expected outcome to compare against to determine the reward.
- Result (result): Specifies the type of evaluation result expected. Here, it checks if the VLC player's recording file path is set to the Desktop.

Together, the instruction and config define the task. The agent needs to modify the VLC player's recording path to the Desktop. The evaluator checks if this modification is successful, providing the agent with the correct reward based on the outcome. The result key, while not mandatory for all tasks, is used to specify the expected evaluation result type.
