import importlib.util
from pathlib import Path

import pytest

MODULE_PATH = Path(__file__).resolve().parents[1] / 'cua_agent' / 'loops' / 'coordinate_parser.py'
spec = importlib.util.spec_from_file_location('coordinate_parser', MODULE_PATH)
coordinate_parser = importlib.util.module_from_spec(spec)
spec.loader.exec_module(coordinate_parser)
parse_uitars_coordinates = coordinate_parser.parse_uitars_coordinates


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("(100, 200)", (100.0, 200.0, 100.0, 200.0)),
        ("[100, 200]", (100.0, 200.0, 100.0, 200.0)),
        ("[100.5, 200.25]", (100.5, 200.25, 100.5, 200.25)),
        ("(100, 200, 300, 400)", (100.0, 200.0, 300.0, 400.0)),
    ],
)
def test_parse_uitars_coordinates_accepts_numeric_points_and_boxes(raw, expected):
    assert parse_uitars_coordinates(raw) == expected


@pytest.mark.parametrize(
    "raw",
    [
        "__import__('os').system('echo pwned')",
        "open('/tmp/x','w')",
        "{'x': object()}",
        "[100, 'bad']",
        "[100]",
        "[100, 200, 300, 400, 500]",
        "[True, 200]",
        "[1e309, 200]",
        "[float('nan'), 200]",
    ],
)
def test_parse_uitars_coordinates_rejects_malformed_or_executable_payloads(raw):
    with pytest.raises(ValueError):
        parse_uitars_coordinates(raw)
