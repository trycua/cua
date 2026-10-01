"""Unity game engine workflow tasks for cua-bench."""

import cua_bench as cb


@cb.tasks_config(split="train")
def load():
    """Define Unity workflow task variants."""
    tasks = [
        {
            "task_type": "create_project",
            "project_name": "MyFirstGame",
            "template": "3D",
            "description": "Create a new Unity 3D project called 'MyFirstGame' and save it to ~/Desktop/UnityProjects/",
        },
        {
            "task_type": "create_cube",
            "project_name": "CubeDemo",
            "object_name": "Player",
            "description": "Create a new Unity project called 'CubeDemo', add a Cube GameObject renamed to 'Player', and save it to ~/Desktop/UnityProjects/",
        },
        {
            "task_type": "create_script",
            "project_name": "ScriptDemo",
            "script_name": "PlayerController",
            "description": "Create a new Unity project called 'ScriptDemo', create a C# script named 'PlayerController', and save it to ~/Desktop/UnityProjects/",
        },
        {
            "task_type": "create_scene",
            "project_name": "SceneDemo",
            "scene_name": "MainMenu",
            "description": "Create a new Unity project called 'SceneDemo', create a new scene called 'MainMenu', and save it to ~/Desktop/UnityProjects/",
        },
        {
            "task_type": "create_ui",
            "project_name": "UIDemo",
            "text_content": "Hello World",
            "description": "Create a new Unity project called 'UIDemo', add a UI Canvas with a Text element saying 'Hello World', and save it to ~/Desktop/UnityProjects/",
        },
        {
            "task_type": "create_scene_objects",
            "project_name": "GameWorld",
            "objects": ["Cube", "Sphere", "Plane"],
            "description": "Create a new Unity project called 'GameWorld', add a Cube, Sphere, and Plane to the scene, and save it to ~/Desktop/UnityProjects/",
        },
        {
            "task_type": "create_project_2d",
            "project_name": "My2DGame",
            "template": "2D",
            "description": "Create a new Unity 2D project called 'My2DGame' and save it to ~/Desktop/UnityProjects/",
        },
        {
            "task_type": "create_with_settings",
            "project_name": "CUAGame",
            "company_name": "CUA Games",
            "description": "Create a new Unity project called 'CUAGame', set the Company Name to 'CUA Games' in Project Settings, and save it to ~/Desktop/UnityProjects/",
        },
    ]

    return [
        cb.Task(
            description=task["description"],
            metadata=task,
            computer={
                "provider": "native",
                "setup_config": {
                    # "os_type": "windows",
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
    """Set up Unity environment by installing Unity Hub."""
    await session.apps.unity.install(with_shortcut=True)


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    """Evaluate if the Unity task was completed correctly."""
    task_type = task_cfg.metadata.get("task_type", "")
    project_name = task_cfg.metadata.get("project_name", "")
    score = 0.0

    # Check if project directory exists
    result = await session.run_command(
        f'if exist "%USERPROFILE%\\Desktop\\UnityProjects\\{project_name}\\Assets" (echo FOUND) else (echo NOT_FOUND)',
        check=False,
    )
    if "FOUND" not in result["stdout"]:
        return [0.0]

    # Base score for having the project created
    score = 0.5

    if task_type == "create_project":
        score = 1.0

    elif task_type == "create_project_2d":
        result = await session.run_command(
            f'findstr /C:"m_ActiveInputHandler" "%USERPROFILE%\\Desktop\\UnityProjects\\{project_name}\\ProjectSettings\\ProjectSettings.asset" 2>nul || echo "NOT_FOUND"',
            check=False,
        )
        if "NOT_FOUND" not in result["stdout"]:
            score = 1.0

    elif task_type == "create_cube":
        result = await session.run_command(
            f'dir /S /B "%USERPROFILE%\\Desktop\\UnityProjects\\{project_name}\\Assets\\*.unity" 2>nul || echo "NO_SCENES"',
            check=False,
        )
        if "NO_SCENES" not in result["stdout"] and ".unity" in result["stdout"]:
            score = 0.75
            object_name = task_cfg.metadata.get("object_name", "Player")
            result = await session.run_command(
                f'findstr /S /C:"{object_name}" "%USERPROFILE%\\Desktop\\UnityProjects\\{project_name}\\Assets\\*.unity" 2>nul || echo "NOT_FOUND"',
                check=False,
            )
            if "NOT_FOUND" not in result["stdout"]:
                score = 1.0

    elif task_type == "create_script":
        script_name = task_cfg.metadata.get("script_name", "PlayerController")
        result = await session.run_command(
            f'if exist "%USERPROFILE%\\Desktop\\UnityProjects\\{project_name}\\Assets\\{script_name}.cs" (echo FOUND) else (echo NOT_FOUND)',
            check=False,
        )
        if "FOUND" in result["stdout"]:
            score = 1.0
        else:
            result = await session.run_command(
                f'dir /S /B "%USERPROFILE%\\Desktop\\UnityProjects\\{project_name}\\Assets\\*.cs" 2>nul || echo "NO_SCRIPTS"',
                check=False,
            )
            if "NO_SCRIPTS" not in result["stdout"]:
                score = 0.75

    elif task_type == "create_scene":
        scene_name = task_cfg.metadata.get("scene_name", "MainMenu")
        result = await session.run_command(
            f'dir /S /B "%USERPROFILE%\\Desktop\\UnityProjects\\{project_name}\\Assets\\*{scene_name}*.unity" 2>nul || echo "NOT_FOUND"',
            check=False,
        )
        if "NOT_FOUND" not in result["stdout"] and ".unity" in result["stdout"]:
            score = 1.0

    elif task_type == "create_ui":
        result = await session.run_command(
            f'findstr /S /C:"Canvas" "%USERPROFILE%\\Desktop\\UnityProjects\\{project_name}\\Assets\\*.unity" 2>nul || echo "NOT_FOUND"',
            check=False,
        )
        if "NOT_FOUND" not in result["stdout"]:
            score = 0.85
            text_content = task_cfg.metadata.get("text_content", "Hello World")
            result = await session.run_command(
                f'findstr /S /C:"{text_content}" "%USERPROFILE%\\Desktop\\UnityProjects\\{project_name}\\Assets\\*.unity" 2>nul || echo "NOT_FOUND"',
                check=False,
            )
            if "NOT_FOUND" not in result["stdout"]:
                score = 1.0

    elif task_type == "create_scene_objects":
        objects = task_cfg.metadata.get("objects", [])
        found_count = 0
        for obj in objects:
            result = await session.run_command(
                f'findstr /S /C:"{obj}" "%USERPROFILE%\\Desktop\\UnityProjects\\{project_name}\\Assets\\*.unity" 2>nul || echo "NOT_FOUND"',
                check=False,
            )
            if "NOT_FOUND" not in result["stdout"]:
                found_count += 1
        if objects:
            score = 0.5 + (0.5 * found_count / len(objects))

    elif task_type == "create_with_settings":
        company_name = task_cfg.metadata.get("company_name", "CUA Games")
        result = await session.run_command(
            f'findstr /C:"{company_name}" "%USERPROFILE%\\Desktop\\UnityProjects\\{project_name}\\ProjectSettings\\ProjectSettings.asset" 2>nul || echo "NOT_FOUND"',
            check=False,
        )
        if "NOT_FOUND" not in result["stdout"]:
            score = 1.0

    return [min(score, 1.0)]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    """Oracle solution for Unity tasks."""
    raise NotImplementedError(
        "Unity tasks require complex UI interaction. "
        "Use this task for agent evaluation rather than oracle solving."
    )


if __name__ == "__main__":
    cb.interact(__file__)
