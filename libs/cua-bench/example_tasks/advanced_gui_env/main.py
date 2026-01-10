import cua_bench as cb


@cb.tasks_config(split="train")
def load():
    return [
        cb.Task(
            description="Test folder serving with webtop provider on macOS",
            metadata={"use_folder": True},
            computer={
                "provider": "simulated",
                "setup_config": {
                    "os_type": "win7",
                    "width": 1280,
                    "height": 800,
                    "background": "#f0f0f0",
                },
            },
        ),
        cb.Task(
            description="Test folder serving with computer provider on Linux",
            metadata={"use_folder": True},
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": "linux",
                    "width": 1280,
                    "height": 800,
                    "background": "#f0f0f0",
                },
            },
        ),
        cb.Task(
            description="Test inline HTML serving with webtop provider on macOS",
            metadata={"use_folder": False},
            computer={
                "provider": "simulated",
                "setup_config": {
                    "os_type": "macos",
                    "width": 1280,
                    "height": 800,
                    "background": "#f0f0f0",
                },
            },
        ),
        cb.Task(
            description="Test inline HTML serving with computer provider on Linux",
            metadata={"use_folder": False},
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": "linux",
                    "width": 1280,
                    "height": 800,
                    "background": "#f0f0f0",
                },
            },
        ),
    ]


@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession | cb.MobileSession):
    from pathlib import Path

    # Get use_folder from task metadata
    use_folder = task_cfg.metadata.get("use_folder", True)  # type: ignore

    if use_folder:
        # Launch window using folder parameter
        folder_path = Path(__file__).parent / "web"

        await session.launch_window(
            folder=str(folder_path),
            title="Folder Example",
            x=100,
            y=100,
            width=800,
            height=600,
        )
    else:
        # Launch window using html parameter with complete.html
        complete_html_path = Path(__file__).parent / "web" / "complete.html"
        html_content = complete_html_path.read_text(encoding="utf-8")

        await session.launch_window(
            html=html_content,
            title="Inline HTML Example",
            x=100,
            y=100,
            width=800,
            height=600,
        )


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession | cb.MobileSession):
    # Simple evaluation - just check if we completed successfully
    return [1.0]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession | cb.MobileSession):
    # Just mark as done
    await session.execute_action(cb.DoneAction())
