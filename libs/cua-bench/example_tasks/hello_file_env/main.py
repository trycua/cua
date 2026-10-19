"""A tiny native task for smoke-testing a target: write a word to a file.

Runs on any Linux image with cua-spacesd (shell and files), for example:

    cb run example_tasks/hello_file_env                    # local gVisor container
    cb run example_tasks/hello_file_env --on cloud --image <registry ref>
"""

import cua_bench as cb

WORDS = ["hello", "bench"]


@cb.tasks_config(split="train")
def load():
    return [
        cb.Task(
            description=f"Write the word '{word}' into /tmp/cb_answer.txt.",
            metadata={"word": word},
            computer={"provider": "native", "setup_config": {"os_type": "linux"}},
        )
        for word in WORDS
    ]


@cb.setup_task(split="train")
async def setup(task_cfg, session: cb.DesktopSession):
    await session.run_command("rm -f /tmp/cb_answer.txt", check=False)
    await session.write_file("/tmp/cb_goal.txt", task_cfg.metadata["word"])


@cb.solve_task(split="train")
async def solve(task_cfg, session: cb.DesktopSession):
    await session.run_command("cp /tmp/cb_goal.txt /tmp/cb_answer.txt")


@cb.evaluate_task(split="train")
async def evaluate(task_cfg, session: cb.DesktopSession) -> list[float]:
    if not await session.file_exists("/tmp/cb_answer.txt"):
        return [0.0]
    answer = (await session.read_file("/tmp/cb_answer.txt")).strip()
    return [1.0 if answer == task_cfg.metadata["word"] else 0.0]
