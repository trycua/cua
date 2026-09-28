"""Minimal, dependency-free protobuf wire-format reader.

AndroidControl's `accessibility_trees` field is a serialized
`android_env.AndroidAccessibilityForest` proto (see
https://github.com/google-deepmind/android_env/blob/main/android_env/proto/a11y/).
The canonical way to read it is `pip install android_env` + generated
`_pb2.py` modules, which pull in a heavy dependency tree (dm-env, grpc, etc.)
this package doesn't otherwise need, and generating bindings requires a
`protoc` toolchain this package does not want to depend on.

Since proto3's wire format is self-describing enough given only the *field
numbers and types* (which are public, documented, and copied verbatim into
`androidcontrol.py`'s comments from the upstream .proto files), this module
implements a small generic wire-format walker instead: no schema compiler,
no generated code, just tag/varint/length-delimited parsing. This is a
one-purpose, hand-rolled reader -- not a general protobuf library -- and it
is only as correct as the field-number mapping the caller supplies.
"""
from __future__ import annotations

# Wire types (proto3 spec).
WT_VARINT = 0
WT_FIXED64 = 1
WT_LEN = 2
WT_FIXED32 = 5


def parse_message(data: bytes) -> dict[int, list[tuple[int, object]]]:
    """Returns {field_number: [(wire_type, raw_value), ...]} -- raw_value is
    an int for varint/fixed32/fixed64, bytes for length-delimited fields."""
    out: dict[int, list[tuple[int, object]]] = {}
    i, n = 0, len(data)
    while i < n:
        tag, i = _read_varint(data, i)
        field_no, wire_type = tag >> 3, tag & 0x7
        if wire_type == WT_VARINT:
            val, i = _read_varint(data, i)
        elif wire_type == WT_LEN:
            length, i = _read_varint(data, i)
            val = data[i:i + length]
            i += length
        elif wire_type == WT_FIXED64:
            val = int.from_bytes(data[i:i + 8], "little")
            i += 8
        elif wire_type == WT_FIXED32:
            val = int.from_bytes(data[i:i + 4], "little")
            i += 4
        else:
            raise ValueError(f"unsupported wire type {wire_type} at byte {i}")
        out.setdefault(field_no, []).append((wire_type, val))
    return out


def _read_varint(data: bytes, i: int) -> tuple[int, int]:
    result, shift = 0, 0
    while True:
        b = data[i]
        i += 1
        result |= (b & 0x7F) << shift
        if not (b & 0x80):
            return result, i
        shift += 7


def get_str(fields: dict, field_no: int, default: str = "") -> str:
    vals = fields.get(field_no)
    if not vals:
        return default
    return vals[-1][1].decode("utf-8", errors="replace")


def get_int(fields: dict, field_no: int, default: int = 0) -> int:
    vals = fields.get(field_no)
    if not vals:
        return default
    return vals[-1][1]


def get_bool(fields: dict, field_no: int, default: bool = False) -> bool:
    vals = fields.get(field_no)
    if not vals:
        return default
    return bool(vals[-1][1])


def get_msg(fields: dict, field_no: int) -> bytes | None:
    vals = fields.get(field_no)
    if not vals:
        return None
    return vals[-1][1]


def get_repeated_msg(fields: dict, field_no: int) -> list[bytes]:
    return [v for (_wt, v) in fields.get(field_no, [])]
