import json
from pathlib import Path
from common import evaluate_cli, expected, result


def evaluate(bundle, state):
    oracle = json.loads((bundle / "evaluator/oracle.json").read_text())
    values = expected(bundle, state["seed"])
    accounts = state.get(oracle["collection"], [])
    checks = {
        "account_count": len(accounts) == 1,
        "fields": accounts == [values],
        "complete_records": all(set(account) == set(oracle["fields"]) for account in accounts),
    }
    return result(state, bundle, {name: checks[name] for name in oracle["required_checks"]})


if __name__ == "__main__":
    evaluate_cli(Path(__file__).resolve().parents[1], evaluate)
