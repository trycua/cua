import ast
from pathlib import Path

root = Path(__file__).resolve().parent
source = (root / "common.py").read_text()
names = {"expected", "fetch_state", "result", "evaluate_cli"}
imports = "import argparse\nimport json\nimport urllib.request\nfrom pathlib import Path\n"
functions = [
    ast.get_source_segment(source, node)
    for node in ast.parse(source).body
    if isinstance(node, ast.FunctionDef) and node.name in names
]
for task in ("web-01", "web-02"):
    (root.parent / task / "evaluator/common.py").write_text(
        imports + "\n\n" + "\n\n\n".join(functions) + "\n"
    )
