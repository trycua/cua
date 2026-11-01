import argparse
import json
import urllib.request
from pathlib import Path


def expected(bundle, seed):
    variants = json.loads((bundle / "fixture/scenario.json").read_text())["variants"]
    return variants[seed % len(variants)]


def fetch_state(workspace, bundle):
    port = json.loads((bundle / "fixture/scenario.json").read_text())["port"]
    trial_path = workspace / ".runtime/trial.json"
    if trial_path.exists():
        port = int(json.loads(trial_path.read_text())["base_url"].rsplit(":", 1)[1])
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/__state", timeout=5) as response:
        return json.load(response)


def result(state, bundle, checks):
    seed = state.get("seed")
    variants = json.loads((bundle / "fixture/scenario.json").read_text())["variants"]
    checks["identity"] = (
        type(seed) is int
        and state.get("task") == bundle.name.upper()
        and state.get("variant_id") == seed % len(variants)
        and state.get("schema_version") == 1
    )
    passed = all(checks.values())
    return dict(
        seed=seed,
        passed=passed,
        score=int(passed),
        detail={"checks": {key: {"passed": bool(value)} for key, value in checks.items()}},
    )


def evaluate_cli(bundle, evaluate):
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path)
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument("--agent-exit-code", type=int, default=0)
    args = parser.parse_args()
    try:
        output = evaluate(bundle, fetch_state(args.workspace, bundle))
    except (OSError, ValueError, KeyError, TypeError) as error:
        output = dict(
            passed=False,
            score=0,
            detail={"checks": {"read_state": {"passed": False, "error": str(error)}}},
        )
    args.result.parent.mkdir(parents=True, exist_ok=True)
    args.result.write_text(json.dumps(output) + "\n")
    print(json.dumps(output))
