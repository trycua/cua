#!/usr/bin/env python3
"""Minimal uv run example: send two batches to the same lane VMs.

Run:
    uv run --with cua-train --extra-index-url https://wheels.cua.ai/simple/ \
      lanes_fan_out.py \
      --client-id key-xxxxxxxx \
      --client-secret your-secret \
      --pool test-pool-please-ignore
"""

from __future__ import annotations

import argparse
import random
import time

from cua_train import TrainClient
from cua_train.models import (
    AcquireLanesRequest,
    Action,
    ActionParameters,
    BatchSubmitRequest,
    ReleaseLanesRequest,
    RunConfig,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Submit two batches to the same lane VMs.")
    parser.add_argument("--client-id", required=True, help="OAuth client id")
    parser.add_argument("--client-secret", required=True, help="OAuth client secret")
    parser.add_argument(
        "--token-url",
        default="https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token",
        help="OIDC token endpoint",
    )
    parser.add_argument("--pool", default="test-pool-please-ignore", help="Target pool name")
    parser.add_argument("--lanes", type=int, default=4, help="Number of lanes (VMs) to acquire")
    parser.add_argument("--steps", type=int, default=5, help="Clicks per run")
    parser.add_argument("--run-timeout", type=int, default=300, help="Run timeout seconds")
    parser.add_argument("--poll-interval", type=float, default=2.0, help="Poll interval seconds")
    return parser.parse_args()


def random_click() -> Action:
    return Action(
        action_type="CLICK",
        parameters=ActionParameters.from_dict(
            {"x": random.randint(200, 1720), "y": random.randint(100, 980), "button": "left"}
        ),
    )


def wait_for_label(client: TrainClient, pool: str, label: str, poll_interval: float) -> list:
    res = client.get_label_results(pool=pool, label=label, completed_only=True)
    while True:
        runs = [r for b in res.batches for r in b.runs] if getattr(res, "batches", None) else []
        if getattr(res, "all_completed", False) and runs:
            return runs
        time.sleep(poll_interval)
        res = client.get_label_results(pool=pool, label=label, completed_only=True)


def submit_and_wait(
    client: TrainClient,
    pool: str,
    lanes: list[str],
    label: str,
    steps: int,
    run_timeout: int,
    poll_interval: float,
) -> tuple[int, int]:
    runs = [RunConfig(vm_id=vm_id, steps=[random_click() for _ in range(steps)], timeout=run_timeout) for vm_id in lanes]

    client.submit_label_batch(
        pool=pool,
        label=label,
        body=BatchSubmitRequest(
            runs=runs,
            concurrency=len(lanes),
        ),
    )
    print(f"submitted {label} with {len(runs)} runs")

    results = wait_for_label(client, pool, label, poll_interval)
    ok = sum(1 for r in results if getattr(r, "ok", False))
    return ok, len(results)


def main() -> None:
    args = parse_args()

    client = TrainClient.from_key(
        token_url=args.token_url,
        client_id=args.client_id,
        client_secret=args.client_secret,
    )

    lanes = client.acquire_lanes(pool=args.pool, body=AcquireLanesRequest(n=args.lanes)).vm_ids
    print(f"acquired {len(lanes)} lanes")

    try:
        first_label = f"uv-example-{int(time.time())}-1"
        first_ok, first_total = submit_and_wait(
            client,
            args.pool,
            lanes,
            first_label,
            args.steps,
            args.run_timeout,
            args.poll_interval,
        )
        print(f"first batch completed: {first_ok}/{first_total} ok")

        # Reuses the exact same vm_ids from the acquired lanes for a second interaction.
        second_label = f"uv-example-{int(time.time())}-2"
        second_ok, second_total = submit_and_wait(
            client,
            args.pool,
            lanes,
            second_label,
            args.steps,
            args.run_timeout,
            args.poll_interval,
        )
        print(f"second batch completed: {second_ok}/{second_total} ok")
    finally:
        released = client.release_lanes(pool=args.pool, body=ReleaseLanesRequest(vm_ids=lanes)).released
        print(f"released {released} lanes")


if __name__ == "__main__":
    main()
