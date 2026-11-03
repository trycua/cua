"""Keep one Cua-S1-4B chooser resident and answer decisions over stdio.

The jev-use chooser (`choose_decision.py --model s1`) loads the model for every
decision, which takes tens of seconds on CPU. Live loops must decide within the
Cua Driver's 60-second capture lifetime, so the S1 docs recommend keeping one
loaded model resident. This helper does exactly that for CI and the desktop
E2E row, using the repository's validation and scoring code unchanged
(`choose_action.validate_request`, `decision_models.S1DecisionModel`,
`decision_models.choose`). It never dispatches actions.

Configuration is the chooser's: `S1_BASE_MODEL_PATH`, `S1_ADAPTER_PATH`,
`S1_DEVICE`, `S1_DTYPE`, and `S1_MODALITY` (one modality per process).
`S1_RESIDENT=0` keeps the weights memory-mapped instead of copying them into
process memory (see `make_resident`).

Serve mode (default) prints one `{"ready": true, ...}` line after loading,
then reads one JSON object per stdin line:

    {"request": <cua.jev_choice_request_v1>, "screenshot": "<png path>"}

`screenshot` is required for multimodal and is bound to the request's
capture. Each reply is one line: `{"decision": <cua.decision_choice_v1>,
"latency_ms": <int>}` or `{"error": "<detail>"}`.

Smoke mode (`--smoke`) runs the checked-in positive and negative fixtures
(with their screenshots for multimodal), checks them exactly as
`verify_decision_cli.py` does, prints one JSON report, and exits nonzero on
any mismatch.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import resource
import sys
import time
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[3]
JEV_USE = REPO / "libs/cua-driver/examples/jev-use"
sys.path.insert(0, str(JEV_USE / "python"))

from choose_action import MAX_INPUT_BYTES, validate_request  # noqa: E402
from choose_decision import _resolve_s1_adapter  # noqa: E402
from decision_models import (  # noqa: E402
    S1_MODALITIES,
    DecisionRequest,
    S1DecisionModel,
    choose,
    s1_model_identity,
)

FIXTURES = {
    "positive": ("jev-choice-request-v1", "submit-form"),
    "negative": ("jev-choice-negative-v1", "abstain"),
}
RESPONSE_KEYS = {
    "schema",
    "kind",
    "capture_id",
    "selected_id",
    "model",
    "confidence",
    "probabilities",
    "reason",
}


def peak_rss_mb() -> float:
    peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    # Linux reports KiB; macOS reports bytes.
    return round(peak / (1024 * 1024 if sys.platform == "darwin" else 1024), 1)


def make_resident(model: Any) -> None:
    """Copy memory-mapped weights into process memory.

    Transformers maps the safetensors shards and loads same-dtype CPU weights
    as file-backed pages. Under desktop memory pressure (Driver, browser,
    OmniParser worker, video recorder) the kernel evicts those clean pages,
    and every forward pass rereads up to 9 GB from disk. Anonymous copies stay
    resident; one tensor is copied at a time, so the peak cost is one tensor.
    """
    import torch

    with torch.no_grad():
        for tensor in [*model.parameters(), *model.buffers()]:
            tensor.data = tensor.data.clone()


def rss_breakdown_mb() -> dict[str, float]:
    """Current anonymous and file-backed resident memory (Linux only)."""
    try:
        status = Path("/proc/self/status").read_text(encoding="utf-8")
    except OSError:
        return {}
    values = {}
    for line in status.splitlines():
        key, _, rest = line.partition(":")
        if key in {"RssAnon", "RssFile"}:
            values[key.lower().replace("rss", "rss_") + "_mb"] = round(
                int(rest.split()[0]) / 1024, 1
            )
    return values


class WarmChooser:
    def __init__(self) -> None:
        base = Path(os.environ["S1_BASE_MODEL_PATH"]).expanduser()
        adapter = Path(os.environ["S1_ADAPTER_PATH"]).expanduser()
        if not base.is_dir() or not adapter.is_dir():
            raise ValueError("S1_BASE_MODEL_PATH and S1_ADAPTER_PATH must be local directories")
        self.modality = os.environ.get("S1_MODALITY", "").strip() or "text"
        if self.modality not in S1_MODALITIES:
            raise ValueError(f"S1_MODALITY must be one of {', '.join(S1_MODALITIES)}")
        _resolve_s1_adapter(adapter, self.modality)
        self.device = os.environ.get("S1_DEVICE", "cpu")
        self.dtype = os.environ.get("S1_DTYPE", "bfloat16")
        self.name = s1_model_identity(
            adapter,
            self.modality,
            repo_id=os.environ.get("S1_ADAPTER_ID"),
            revision=os.environ.get("S1_ADAPTER_REVISION"),
        )
        from cua_s1.four_b import FourBModel

        started = time.perf_counter()
        self.scorer = FourBModel(
            base_model=str(base),
            lora_adapter_path=adapter,
            device=self.device,
            dtype=self.dtype,
            modality=self.modality,
        )
        self.scorer.load()
        self.load_s = round(time.perf_counter() - started, 2)
        self.resident_s: float | None = None
        self.warmup_ms: int | None = None
        if os.environ.get("S1_RESIDENT", "1") != "0":
            started = time.perf_counter()
            make_resident(self.scorer._model)
            self.resident_s = round(time.perf_counter() - started, 2)

    def decide(self, raw: Any, screenshot: str | None) -> tuple[dict[str, Any], int]:
        if len(json.dumps(raw).encode()) > MAX_INPUT_BYTES:
            raise ValueError("request exceeds input limit")
        request = DecisionRequest.from_validated(validate_request(raw))
        shot = None
        if self.modality == "multimodal":
            if not screenshot or not Path(screenshot).is_file():
                raise ValueError("multimodal S1 requires an existing screenshot path")
            shot = Path(screenshot)
        elif screenshot:
            raise ValueError("screenshot requires S1_MODALITY=multimodal")
        model = S1DecisionModel(
            self.scorer,
            modality=self.modality,
            screenshot_path=shot,
            screenshot_capture_id=request.capture_id if shot else None,
            name=self.name,
        )
        started = time.perf_counter()
        result = choose(model, request).to_wire()
        return result, round((time.perf_counter() - started) * 1000)

    def describe(self) -> dict[str, Any]:
        import torch

        return {
            "model": self.name,
            "modality": self.modality,
            "device": self.device,
            "dtype": self.dtype,
            "load_s": self.load_s,
            "resident_s": self.resident_s,
            "warmup_ms": self.warmup_ms,
            "torch": torch.__version__,
            "torch_threads": torch.get_num_threads(),
            "cpu_count": os.cpu_count(),
            "machine": platform.machine(),
            "peak_rss_mb": peak_rss_mb(),
            **rss_breakdown_mb(),
        }


def check_response(response: dict[str, Any], request: dict[str, Any], expected_id: str) -> None:
    """The same checks `verify_decision_cli.py` applies to the chooser output."""
    if set(response) != RESPONSE_KEYS or response["schema"] != "cua.decision_choice_v1":
        raise RuntimeError("decision model returned an unsupported response")
    if response["capture_id"] != request["capture_id"]:
        raise RuntimeError("decision model returned a different capture")
    expected_kind = expected_id if expected_id in {"reobserve", "abstain"} else "selected"
    if (response["kind"], response["selected_id"]) != (expected_kind, expected_id):
        raise RuntimeError(
            f"decision model chose {response['kind']}/{response['selected_id']}, "
            f"expected {expected_kind}/{expected_id}"
        )
    if set(response["probabilities"]) != {item["id"] for item in request["candidates"]}:
        raise RuntimeError("decision model returned a different candidate set")


def smoke(chooser: WarmChooser, repeats: int) -> int:
    report: dict[str, Any] = {**chooser.describe(), "decisions": []}
    failures = 0
    for fixture, (stem, expected_id) in FIXTURES.items():
        request = json.loads((JEV_USE / "fixtures" / f"{stem}.json").read_text(encoding="utf-8"))
        screenshot = (
            str(JEV_USE / "fixtures" / f"{stem}.png") if chooser.modality == "multimodal" else None
        )
        for attempt in range(1, repeats + 1):
            entry: dict[str, Any] = {
                "fixture": fixture,
                "attempt": attempt,
                "expected": expected_id,
            }
            try:
                response, latency_ms = chooser.decide(request, screenshot)
                entry.update(
                    latency_ms=latency_ms,
                    kind=response["kind"],
                    selected_id=response["selected_id"],
                    confidence=response["confidence"],
                )
                check_response(response, request, expected_id)
                entry["ok"] = True
            except Exception as error:  # noqa: BLE001 - reported, then fails the run
                entry.update(ok=False, error=str(error)[:300])
                failures += 1
            report["decisions"].append(entry)
            print(json.dumps(entry), file=sys.stderr, flush=True)
    report["peak_rss_mb"] = peak_rss_mb()
    report["ok"] = failures == 0
    print(json.dumps(report, separators=(",", ":")), flush=True)
    return 0 if failures == 0 else 1


def serve(chooser: WarmChooser, out: Any) -> int:
    def reply(value: dict[str, Any]) -> None:
        out.write(json.dumps(value, separators=(",", ":")) + "\n")
        out.flush()

    reply({"ready": True, **chooser.describe()})
    for line in sys.stdin:
        if not line.strip():
            continue
        try:
            message = json.loads(line)
            if not isinstance(message, dict) or "request" not in message:
                raise ValueError("expected an object with a request")
            decision, latency_ms = chooser.decide(message["request"], message.get("screenshot"))
            reply({"decision": decision, "latency_ms": latency_ms, "peak_rss_mb": peak_rss_mb()})
        except (ValueError, KeyError, TypeError, json.JSONDecodeError) as error:
            reply({"error": str(error)[:300]})
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--smoke", action="store_true", help="run the fixture checks and exit")
    parser.add_argument("--repeats", type=int, default=2, help="decisions per fixture in --smoke")
    parser.add_argument(
        "--warmup",
        action="store_true",
        help="serve mode: run one positive-fixture decision before reporting ready",
    )
    args = parser.parse_args(argv)

    # Keep the protocol stream clean: libraries may print while loading.
    protocol = sys.stdout
    sys.stdout = sys.stderr
    try:
        chooser = WarmChooser()
    except (KeyError, ValueError, ImportError) as error:
        protocol.write(json.dumps({"ready": False, "error": f"S1 setup failed: {error}"}) + "\n")
        protocol.flush()
        return 2
    if args.smoke:
        sys.stdout = protocol
        return smoke(chooser, max(1, args.repeats))
    if args.warmup:
        stem, _ = FIXTURES["positive"]
        request = json.loads((JEV_USE / "fixtures" / f"{stem}.json").read_text(encoding="utf-8"))
        screenshot = (
            str(JEV_USE / "fixtures" / f"{stem}.png") if chooser.modality == "multimodal" else None
        )
        _, latency_ms = chooser.decide(request, screenshot)
        chooser.warmup_ms = latency_ms
        print(f"warmup decision: {latency_ms} ms", file=sys.stderr, flush=True)
    return serve(chooser, protocol)


if __name__ == "__main__":
    raise SystemExit(main())
