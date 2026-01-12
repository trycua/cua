from pathlib import Path

import cua_bench as cb


# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"]  # ["macos", "win11", "win10"]

    # Define different form filling scenarios
    form_data_scenarios = [
        {
            "name": "John Smith",
            "email": "john.smith@example.com",
            "age": "25",
            "country": "USA",
            "comments": "I would like to receive updates.",
            "subscribe": True,
            "gender": "male",
        },
        {
            "name": "Jane Doe",
            "email": "jane.doe@example.com",
            "age": "30",
            "country": "Canada",
            "comments": "Please contact me regarding new products.",
            "subscribe": False,
            "gender": "female",
        },
        {
            "name": "Alex Johnson",
            "email": "alex.j@example.com",
            "age": "28",
            "country": "UK",
            "comments": "Looking forward to hearing from you!",
            "subscribe": True,
            "gender": "other",
        },
    ]

    return [
        cb.Task(
            description=f'Fill out the form with the following information: Name: "{form_data["name"]}", Email: "{form_data["email"]}", Age: {form_data["age"]}, Country: "{form_data["country"]}", Comments: "{form_data["comments"]}", Subscribe to newsletter: {"Yes" if form_data["subscribe"] else "No"}, Gender: "{form_data["gender"]}". Then submit the form.',
            metadata={**form_data},
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": os_type,
                    "width": 1024,
                    "height": 768,
                    "background": "#c0c0c0",
                },
            },
        )
        for os_type in os_types
        for form_data in form_data_scenarios
    ]


# All code below will be running in a separate process per task

pid = None


# Called at start of task
@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Setup steps:
    # 1. Create a webview window
    pid = await session.launch_window(
        html=(Path(__file__).parent / "gui/index.html").read_text("utf-8"),
        title="Form Submission",
        width=600,
        height=500,
    )


# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Check if the form was submitted and data matches
    form_data = await session.execute_javascript(pid, "window.__formData")

    if form_data is None:
        return [0.0]

    # Check if all required fields match the expected values
    expected = task_cfg.metadata

    fields_match = (
        form_data.get("name") == expected["name"]
        and form_data.get("email") == expected["email"]
        and form_data.get("age") == expected["age"]
        and form_data.get("country") == expected["country"]
        and form_data.get("comments") == expected["comments"]
        and form_data.get("subscribe") == expected["subscribe"]
        and form_data.get("gender") == expected["gender"]
    )

    # Return full reward if all fields match, 0 otherwise
    return [1.0] if fields_match else [0.0]


# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Solution steps: Fill out the form using the bot helpers
    metadata = task_cfg.metadata

    # Fill in text fields
    await session.click_element(pid, "#name")
    await session.execute_action(cb.TypeAction(text=metadata["name"]))

    await session.click_element(pid, "#email")
    await session.execute_action(cb.TypeAction(text=metadata["email"]))

    await session.click_element(pid, "#age")
    await session.execute_action(cb.TypeAction(text=metadata["age"]))

    # Select country from dropdown
    await session.click_element(pid, "#country")
    await session.click_element(pid, f'option[value="{metadata["country"]}"]')

    # Select gender radio button
    await session.click_element(pid, f'input[name="gender"][value="{metadata["gender"]}"]')

    # Handle checkbox
    if metadata["subscribe"]:
        await session.click_element(pid, "#subscribe")

    # Fill in comments textarea
    await session.click_element(pid, "#comments")
    await session.execute_action(cb.TypeAction(text=metadata["comments"]))

    # Submit the form
    await session.click_element(pid, "#submit-btn")


if __name__ == "__main__":
    cb.interact(__file__)
