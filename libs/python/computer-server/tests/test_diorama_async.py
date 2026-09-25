import ast
from pathlib import Path


DIORAMA_SOURCE = Path(__file__).parents[1] / "computer_server" / "diorama" / "diorama.py"


def test_mouse_tracking_throttle_yields_to_the_event_loop():
    tree = ast.parse(DIORAMA_SOURCE.read_text())
    main = next(
        node for node in tree.body if isinstance(node, ast.AsyncFunctionDef) and node.name == "main"
    )

    blocking_sleeps = [
        node
        for node in ast.walk(main)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Attribute)
        and isinstance(node.func.value, ast.Name)
        and node.func.value.id == "time"
        and node.func.attr == "sleep"
    ]
    awaited_throttles = [
        node
        for node in ast.walk(main)
        if isinstance(node, ast.Await)
        and isinstance(node.value, ast.Call)
        and isinstance(node.value.func, ast.Attribute)
        and isinstance(node.value.func.value, ast.Name)
        and node.value.func.value.id == "asyncio"
        and node.value.func.attr == "sleep"
        and len(node.value.args) == 1
        and isinstance(node.value.args[0], ast.Constant)
        and node.value.args[0].value == 0.05
    ]

    assert blocking_sleeps == []
    assert len(awaited_throttles) == 1
