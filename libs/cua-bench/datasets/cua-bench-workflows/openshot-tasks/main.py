"""OpenShot video editor workflow tasks for cua-bench."""

import json

import cua_bench as cb

# Title templates with their sodipodi:docname identifiers for verification
TITLE_TEMPLATES = {
    "Bar 1": "Bar_1.svg",
    "Bar 2": "Bar_2.svg",
    "Bar 3": "Bar_3.svg",
    "Box": "Box.svg",
    "Bubbles 1": "Bubbles_1.svg",
    "Bubbles 2": "Bubbles_2.svg",
    "Camera Border": "Camera_Border.svg",
    "Cloud 1": "Cloud_1.svg",
    "Cloud 2": "Cloud_2.svg",
    "Creative Commons 1": "Creative_Commons_1.svg",
    "Creative Commons 2": "Creative_Commons_2.svg",
    "Film Rating 1": "Film_Rating_1.svg",
    "Film Rating 2": "Film_Rating_2.svg",
    "Film Rating 3": "Film_Rating_3.svg",
    "Film Rating 4": "Film_Rating_4.svg",
    "Flames": "Flames.svg",
    "Flare": "Flare.svg",
    "Footer 1": "Footer_1.svg",
    "Footer 2": "Footer_2.svg",
    "Footer 3": "Footer_3.svg",
    "Gold 1": "Gold_1.svg",
    "Gold 2": "Gold_2.svg",
    "Gold Bottom": "Gold_Bottom.svg",
    "Gold Top": "Gold_Top.svg",
    "Header 1": "Header_1.svg",
    "Header 2": "Header_2.svg",
    "Header 3": "Header_3.svg",
    "Oval 1": "Oval_1.svg",
    "Oval 2": "Oval_2.svg",
    "Oval 3": "Oval_3.svg",
    "Oval 4": "Oval_4.svg",
    "Post it": "Post_it.svg",
    "Ribbon 1": "Ribbon_1.svg",
    "Ribbon 2": "Ribbon_2.svg",
    "Ribbon 3": "Ribbon_3.svg",
    "Smoke 1": "Smoke_1.svg",
    "Smoke 2": "Smoke_2.svg",
    "Smoke 3": "Smoke_3.svg",
    "Solid Color": "Solid_Color.svg",
    "Standard 1": "Standard_1.svg",
    "Standard 2": "Standard_2.svg",
    "Standard 3": "Standard_3.svg",
    "Standard 4": "Standard_4.svg",
}


@cb.tasks_config(split="train")
def load():
    """Define OpenShot workflow task variants."""
    tasks = []

    # Single create_project task
    tasks.append(
        {
            "task_type": "create_project",
            "project_name": "MyFirstVideo",
            "description": "Create a new OpenShot project called 'MyFirstVideo' and save it to ~/Desktop/VideoProjects/",
        }
    )

    # Generate add_title tasks for each title template
    for title_name, svg_filename in TITLE_TEMPLATES.items():
        tasks.append(
            {
                "task_type": "add_title",
                "project_name": f"TitleDemo_{title_name.replace(' ', '_')}",
                "title_template": title_name,
                "title_svg": svg_filename,
                "title_text": "Welcome",
                "description": f"Create a new OpenShot project called 'TitleDemo_{title_name.replace(' ', '_')}', add a '{title_name}' title clip with the text 'Welcome', and save it to ~/Desktop/VideoProjects/",
            }
        )

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
    """Set up OpenShot environment by installing the video editor."""
    # Create the projects directory
    await session.run_command("mkdir -p ~/Desktop/VideoProjects", check=False)

    # Install OpenShot
    await session.apps.openshot.install(with_shortcut=True)


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    """Evaluate if the OpenShot task was completed correctly."""
    task_type = task_cfg.metadata.get("task_type", "")
    project_name = task_cfg.metadata.get("project_name", "")

    if task_type == "create_project":
        # Check if project file exists (.osp is OpenShot project format)
        result = await session.run_command(
            f"test -f ~/Desktop/VideoProjects/{project_name}.osp && echo YES || echo NO",
            check=False,
        )
        if result["stdout"].strip() == "YES":
            return [1.0]
        return [0.0]

    elif task_type == "add_title":
        title_svg = task_cfg.metadata.get("title_svg", "")

        # Check if project file exists
        result = await session.run_command(
            f"test -f ~/Desktop/VideoProjects/{project_name}.osp && echo YES || echo NO",
            check=False,
        )
        project_exists = result["stdout"].strip() == "YES"

        # Check if there's a title SVG asset matching the template
        result = await session.run_command(
            f"cat ~/Desktop/VideoProjects/{project_name}_assets/title/*.svg 2>/dev/null",
            check=False,
        )
        svg_content = result["stdout"]
        docname_match = f'sodipodi:docname="{title_svg}"'
        has_title_asset = docname_match in svg_content

        # Check if target text is in the SVG
        title_text = task_cfg.metadata.get("title_text", "")
        has_target_text = title_text in svg_content

        # Check if any clip references the title asset
        has_clip_reference = False
        if project_exists:
            result = await session.run_command(
                f"cat ~/Desktop/VideoProjects/{project_name}.osp 2>/dev/null",
                check=False,
            )
            try:
                osp = json.loads(result["stdout"])
                clips = osp.get("clips", [])
                has_clip_reference = any(
                    "@assets/title/" in clip.get("reader", {}).get("path", "") for clip in clips
                )
            except (json.JSONDecodeError, KeyError):
                pass

        # Score based on conditions (0.25 each)
        score = 0.0
        if project_exists:
            score = 0.25
            if has_title_asset:
                score = 0.5
                if has_target_text:
                    score = 0.75
                    if has_clip_reference:
                        score = 1.0

        return [score]

    return [0.0]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    """Oracle solution for OpenShot tasks."""
    raise NotImplementedError(
        "OpenShot tasks require complex UI interaction. "
        "Use this task for agent evaluation rather than oracle solving."
    )


if __name__ == "__main__":
    cb.interact(__file__)
