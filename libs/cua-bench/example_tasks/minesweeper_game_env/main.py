from pathlib import Path

import cua_bench as cb


# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"]  # ["macos", "win11", "win10"]
    # Minesweeper game variants: (rows, cols, mines)
    game_configs = [
        (8, 8, 10),  # Easy
        (10, 10, 15),  # Medium
        (12, 12, 20),  # Hard
    ]
    return [
        cb.Task(
            description=f"Play Minesweeper on a {rows}x{cols} grid with {mines} mines. Click cells to reveal them, right-click to flag mines. Win by revealing all non-mine cells.",
            metadata={
                "rows": rows,
                "cols": cols,
                "mines": mines,
            },
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": os_type,
                    "width": 800,
                    "height": 800,
                    "background": "#c0c0c0",
                },
            },
        )
        for os_type in os_types
        for rows, cols, mines in game_configs
    ]


# All code below will be running in a separate process per task

pid = None


# Called at start of task
@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession | cb.MobileSession):
    global pid

    # Setup steps:
    # 1. Create a webview window with Minesweeper game
    rows = task_cfg.metadata["rows"]  # type: ignore
    cols = task_cfg.metadata["cols"]  # type: ignore
    mines = task_cfg.metadata["mines"]  # type: ignore

    # Calculate window size based on grid size (30px per cell + padding)
    window_width = cols * 30 + 100
    window_height = rows * 30 + 200

    pid = await session.launch_window(
        html=(Path(__file__).parent / "gui/index.html").read_text("utf-8"),
        title="Minesweeper",
        width=window_width,
        height=window_height,
    )

    # 2. Initialize the game with the task configuration
    if pid is not None:
        await session.execute_javascript(pid, f"window.initGame({rows}, {cols}, {mines})")


# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession | cb.MobileSession) -> list[float]:
    global pid

    if pid is None:
        return [0.0]

    # Evaluation steps:
    # 1. Check the game state
    game_won = await session.execute_javascript(pid, "window.__gameWon")
    game_lost = await session.execute_javascript(pid, "window.__gameLost")

    # 2. Return reward based on outcome
    # Win: 1.0, Loss: 0.0, In-progress: 0.0
    if game_won is True:
        return [1.0]
    elif game_lost is True:
        return [0.0]
    else:
        # Game still in progress - partial credit based on revealed cells
        rows = task_cfg.metadata["rows"]  # type: ignore
        cols = task_cfg.metadata["cols"]  # type: ignore
        mines = task_cfg.metadata["mines"]  # type: ignore

        revealed_count = await session.execute_javascript(pid, "window.game.revealedCount")
        total_non_mines = rows * cols - mines

        # Partial reward: percentage of safe cells revealed
        if revealed_count and total_non_mines > 0:
            return [revealed_count / total_non_mines]
        return [0.0]


# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession | cb.MobileSession):
    global pid
    import asyncio

    if pid is None:
        return

    # Solution steps:
    # A simple heuristic: click cells starting from corners and edges
    # This is a basic strategy - not optimal but demonstrates bot interaction

    rows = task_cfg.metadata["rows"]  # type: ignore
    cols = task_cfg.metadata["cols"]  # type: ignore

    # Strategy: Start from corner (safer), then expand
    # Click a corner cell first to start the game
    await session.click_element(pid, '[data-row="0"][data-col="0"]')
    await asyncio.sleep(0.5)

    # Check game state after each move
    game_state = await session.execute_javascript(pid, "window.__gameState")
    # Continue clicking unrevealed cells in a pattern
    # This is a simple left-to-right, top-to-bottom scan
    for r in range(rows):
        for c in range(cols):
            if game_state != "playing":
                break

            # Try to click cells that aren't revealed or flagged
            try:
                is_revealed = await session.execute_javascript(
                    pid, f"window.game.revealed[{r}][{c}]"
                )
                is_flagged = await session.execute_javascript(pid, f"window.game.flagged[{r}][{c}]")

                if not is_revealed and not is_flagged:
                    await session.click_element(pid, f'[data-row="{r}"][data-col="{c}"]')
                    await asyncio.sleep(0.2)
                    game_state = await session.execute_javascript(pid, "window.__gameState")

            except Exception:
                # Cell might not exist or game might be over
                continue

        if game_state != "playing":
            break
