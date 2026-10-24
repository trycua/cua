import argparse
import hashlib
import json
import shutil
import urllib.request
from pathlib import Path


def expected(bundle, seed):
    variants = json.loads((bundle / "fixture/scenario.json").read_text())["variants"]
    return variants[seed % len(variants)]


def reset_workspace(bundle, workspace, seed, port):
    shutil.rmtree(workspace, ignore_errors=True)
    runtime = workspace / ".runtime"
    (runtime / "browser").mkdir(parents=True)
    variants = json.loads((bundle / "fixture/scenario.json").read_text())["variants"]
    trial = dict(
        task=bundle.name.upper(),
        seed=seed,
        variant_id=seed % len(variants),
        base_url=f"http://127.0.0.1:{port}",
        values=expected(bundle, seed),
    )
    (runtime / "trial.json").write_text(json.dumps(trial))
    brief = (bundle / "brief.md").read_text().replace("{{url}}", trial["base_url"])
    brief = brief.replace("{{values}}", json.dumps(trial["values"], indent=2))
    (runtime / "brief.md").write_text(brief)


def verify_workspace(bundle, workspace, seed):
    manifest = json.loads((bundle / "task.cuabench.json").read_text())
    inputs = manifest["evaluator"]["inputs"] + [
        entry for variant in manifest["variants"] for entry in variant["fixture_artifacts"]
    ]
    for entry in inputs:
        actual = hashlib.sha256((bundle / entry["path"]).read_bytes()).hexdigest()
        if actual != entry["sha256"]:
            raise ValueError(f"digest mismatch: {entry['path']}")
    trial = json.loads((workspace / ".runtime/trial.json").read_text())
    if trial["seed"] != seed or trial["values"] != expected(bundle, seed):
        raise ValueError("trial mismatch")
    if list((workspace / ".runtime/browser").iterdir()):
        raise ValueError("stale browser profile")


def reset_cli(bundle, verify=False):
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument(
        "--port",
        type=int,
        default=json.loads((bundle / "fixture/scenario.json").read_text())["port"],
    )
    args = parser.parse_args()
    if verify:
        verify_workspace(bundle, args.workspace, args.seed)
    else:
        reset_workspace(bundle, args.workspace, args.seed, args.port)


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
