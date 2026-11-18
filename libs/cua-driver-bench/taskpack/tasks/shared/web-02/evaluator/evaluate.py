import json
from pathlib import Path
from common import evaluate_cli, expected, result


def evaluate(bundle, state):
    oracle = json.loads((bundle / "evaluator/oracle.json").read_text())
    values = expected(bundle, state["seed"])
    orders = state.get(oracle["collection"], [])
    checks = {
        "order_count": len(orders) == 1,
        "items": bool(orders) and all(order.get("items") == values["items"] for order in orders),
        "address": bool(orders)
        and all(order.get("address") == values["address"] for order in orders),
        "card_last4": bool(orders)
        and all(
            order.get("card_last4") == values["card"][-4:] and set(order) == set(oracle["fields"])
            for order in orders
        ),
        "no_draft": state.get("checkout_draft") is None,
    }
    return result(state, bundle, {name: checks[name] for name in oracle["required_checks"]})


if __name__ == "__main__":
    evaluate_cli(Path(__file__).resolve().parents[1], evaluate)
