"""A starter cua-bench task: runs in the Linux container (the default target).

    cb run .                  # local gVisor container, oracle solution
    cb run . --kind vm        # the same task in a local QEMU VM
    cb run . --on cloud       # a Fleet sandbox
    cb interact .             # open the sandbox and try it yourself

Each variant asks for one word in a file. Replace the setup, the oracle and
the checks with your own; HTML pages can be opened with
``session.launch_window(html=...)`` on images that ship bench-ui.
"""

import cua_bench as cb

ANSWER = "/tmp/answer.txt"
WORDS = ["hello", "bench"]


@cb.tasks_config(split="train")
def load():
    """Define task variants."""
    return [
        cb.Task(
            description=f"Create the file {ANSWER} containing only the word '{word}'.",
            metadata={"word": word},
            computer={
                "provider": "native",
                "setup_config": {"os_type": "linux", "width": 1024, "height": 768},
            },
        )
        for word in WORDS
    ]


@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    """Initialize the task environment."""
    await session.run_command(f"rm -f {ANSWER}", check=False)


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    """Return reward based on task completion."""
    if not await session.file_exists(ANSWER):
        return [0.0]
    answer = (await session.read_file(ANSWER)).strip()
    return [1.0 if answer == task_cfg.metadata["word"] else 0.0]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    """Demonstrate the solution (the oracle)."""
    await session.write_file(ANSWER, task_cfg.metadata["word"])
