import cua_bench as cb
from pathlib import Path


@cb.tasks_config(split="train")
def load():
    """Define task variants."""
    qa_pairs = [
        {
            "question": "What is the capital of France?",
            "answer": "Paris"
        },
        {
            "question": "What is 2 + 2?",
            "answer": "4"
        },
        {
            "question": "What color is the sky on a clear day?",
            "answer": "blue"
        },
        {
            "question": "What is the largest planet in our solar system?",
            "answer": "Jupiter"
        }
    ]

    return [
        cb.Task(
            description=f'Open the question.txt file on the Desktop, read the question, and write your answer in answer.txt on the Desktop. Question: {qa["question"]}',
            metadata={
                "question": qa["question"],
                "answer": qa["answer"]
            },
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": "linux",
                    "width": 1024,
                    "height": 768
                }
            }
        )
        for qa in qa_pairs
    ]


@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    """Initialize the task environment."""
    # Get the question from metadata
    question = task_cfg.metadata["question"]

    # Create Desktop directory if it doesn't exist
    await session.run_command("mkdir -p ~/Desktop")

    # Write the question to a text file on the Desktop
    await session.run_command(f'echo "{question}" > ~/Desktop/question.txt')

    # Ensure answer.txt doesn't exist yet
    await session.run_command("rm -f ~/Desktop/answer.txt")


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    """Return reward based on task completion."""
    expected_answer = task_cfg.metadata["answer"].strip().lower()

    # Check if answer.txt exists and read it
    result = await session.run_command("cat ~/Desktop/answer.txt 2>/dev/null || echo ''")

    # Extract the actual answer from the command result
    user_answer = result["stdout"].strip().lower() if result else ""

    # Check if the answer matches (case-insensitive, stripped)
    if user_answer == expected_answer:
        return [1.0]
    else:
        return [0.0]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    """Demonstrate the solution."""
    answer = task_cfg.metadata["answer"]

    # Open the question file to read it (demonstration)
    await session.run_command("cat ~/Desktop/question.txt")

    # Write the answer to answer.txt
    await session.run_command(f'echo "{answer}" > ~/Desktop/answer.txt')
