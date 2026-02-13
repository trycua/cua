"""KiCad workflow tasks for cua-bench.

Simple example: create a new KiCad project and save it. Verification checks
that the project directory and .kicad_pro file exist.
"""

import cua_bench as cb


@cb.tasks_config(split="train")
def load():
    """Define KiCad task variants."""
    tasks = [
        {
            "task_type": "create_project",
            "project_name": "MyFirstBoard",
            "description": (
                "KiCad is already open. Create a new project named 'MyFirstBoard', "
                "save it to the Desktop in a folder named KiCadProjects, then close KiCad."
            ),
        },
        {
            "task_type": "create_project",
            "project_name": "BlinkyPCB",
            "description": (
                "KiCad is already open. Create a new project named 'BlinkyPCB', "
                "save it to the Desktop in a folder named KiCadProjects, then close KiCad."
            ),
        },
    ]

    return [
        cb.Task(
            description=task["description"],
            metadata=task,
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": "linux",
                    "width": 1920,
                    "height": 1080,
                },
            },
        )
        for task in tasks
    ]


@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    """Install KiCad and launch it so the window is visible."""
    await session.apps.kicad.install(with_shortcut=True)
    await session.apps.kicad.launch()


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    """Verify the KiCad project was created: project dir and .kicad_pro file must exist."""
    project_name = task_cfg.metadata.get("project_name", "")
    if not project_name:
        return [0.0]

    os_type = "linux"
    if hasattr(session, "os_type"):
        os_type = session.os_type
    elif hasattr(session, "_config") and session._config:
        os_type = session._config.get("os_type", "linux")
    # Normalize to linux/windows for path checks
    is_windows = os_type in ("windows", "win11", "win10", "win7", "winxp", "win98")

    if is_windows:
        # Windows: Desktop\KiCadProjects\<name>\<name>.kicad_pro
        project_file = f"%USERPROFILE%\\Desktop\\KiCadProjects\\{project_name}\\{project_name}.kicad_pro"
        result = await session.run_command(
            f'if exist "{project_file}" (echo FOUND) else (echo NOT_FOUND)',
            check=False,
        )
    else:
        # Linux/macOS: ~/Desktop/KiCadProjects/<name>/<name>.kicad_pro
        project_file = f"$HOME/Desktop/KiCadProjects/{project_name}/{project_name}.kicad_pro"
        result = await session.run_command(
            f'test -f {project_file} && echo FOUND || echo NOT_FOUND',
            check=False,
        )

    stdout = (result.get("stdout", "") if isinstance(result, dict) else str(result)).strip()
    # Require exact FOUND (avoid NOT_FOUND matching)
    return [1.0] if stdout == "FOUND" else [0.0]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    """Oracle not implemented: KiCad requires GUI interaction to create/save projects.

    No-op so that `cb run task KiCad-task --variant-id 0 --oracle` completes
    (setup runs, evaluate runs and returns 0.0). Use with an agent for real solutions.
    """
    pass


if __name__ == "__main__":
    cb.interact(__file__)
