"""Export schema snapshots: what downstream readers of cua-bench output rely on.

Frozen in ``golden/``:

* ``summary.json`` and per-variant ``result.json``: key sets and value types;
* the trace dataset features (``task_<n>_trace``);
* the ``cb dataset build`` row schemas (aguvis-stage-1, gui-r1).

New keys are fine (additive, regenerate with ``CUA_BENCH_UPDATE_GOLDENS=1``);
a removed or retyped key fails here.
"""

from __future__ import annotations

import json
from pathlib import Path

from .fakes import png_bytes
from .golden_utils import check_golden
from .test_cli_golden import HELLO, cli_env, run_cb  # noqa: F401 - fixture reuse


def _schema(value):
    """Key -> type name, recursively for dicts; lists by their first item."""
    if isinstance(value, dict):
        return {k: _schema(v) for k, v in sorted(value.items())}
    if isinstance(value, list):
        return [_schema(value[0])] if value else []
    return type(value).__name__


def _feature(feature) -> object:
    """A datasets feature as plain data, stable across datasets releases."""
    inner = getattr(feature, "feature", None)
    if inner is not None:  # Sequence / List
        return [_feature(inner)]
    dtype = getattr(feature, "dtype", None)
    name = type(feature).__name__
    return f"{name}:{dtype}" if name == "Value" else name


def test_run_output_schemas(cli_env, tmp_path):  # noqa: F811 - pytest fixture
    out = tmp_path / "out"
    assert run_cb(["run", str(HELLO), "--output-dir", str(out)]) == 0
    summary = json.loads((out / "summary.json").read_text())
    result = json.loads((out / "hello_file_env_v0" / "result.json").read_text())
    assert summary["schema_version"] == 1 and result["schema_version"] == 1
    check_golden("summary_schema", _schema(summary))
    check_golden("result_schema", _schema(result))

    from datasets import load_from_disk

    trace = load_from_disk(str(out / "hello_file_env_v0" / "task_0_trace"))
    check_golden("trace_features", {k: _feature(v) for k, v in trace.features.items()})
    assert [row["event_name"] for row in trace] == ["reset", "solve", "evaluate"]


SNAPSHOT_HTML = """<html><body>
<button aria-label="Submit order" data-instruction="Click the submit button"
  data-bbox-center-hit="true" data-bbox-x="10" data-bbox-y="20"
  data-bbox-width="60" data-bbox-height="20">Submit</button>
<a href="#" aria-label="Help" data-bbox-center-hit="true" data-bbox-x="100"
  data-bbox-y="5" data-bbox-width="30" data-bbox-height="12">Help</a>
</body></html>"""


def _synthetic_outputs(root: Path) -> Path:
    """A trace dir shaped like the ones the processors read (reset + webview)."""
    from cua_bench.tracing import Tracing

    tracing = Tracing(env=None)
    tracing.start()
    tracing.record(
        "reset",
        {
            "task": "Task(description='x')",
            "snapshot": {"windows": [{"window_type": "webview", "html": SNAPSHOT_HTML}]},
            "setup_config": {"os_type": "linux", "width": 200, "height": 100},
        },
        [png_bytes(200, 100)],
    )
    outputs = root / "outputs"
    tracing.save_to_disk(str(outputs / "task_0_trace"))
    return outputs


def test_dataset_build_row_schemas(tmp_path):
    from cua_bench.processors import get_processor
    from cua_bench.processors.base import ProcessorArgs

    outputs = _synthetic_outputs(tmp_path)
    schemas = {}
    for mode in ("aguvis-stage-1", "gui-r1"):
        processor = get_processor(mode)(ProcessorArgs(outputs_path=outputs))
        rows = processor.process()
        assert rows, f"{mode} produced no rows"
        schemas[mode] = {k: type(v).__name__ for k, v in sorted(rows[0].items())}
    check_golden("dataset_build_rows", schemas)
