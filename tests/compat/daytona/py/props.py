"""Property tests (hypothesis) of the wire translation: whatever the SDK is
given, what comes back is what a model of Daytona says. Bounded: at most
HYPOTHESIS_MAX_EXAMPLES (default 20) examples per property, no deadline.

    python props.py --env env.json        (the gateway over mock Spaces)
"""

from __future__ import annotations

import json
import os
import sys

from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st

MAX = int(os.environ.get("HYPOTHESIS_MAX_EXAMPLES", "20"))
SETTINGS = settings(
    max_examples=MAX, deadline=None, suppress_health_check=list(HealthCheck), database=None
)

name_chars = st.characters(blacklist_categories=("Cs", "Cc"), blacklist_characters="/\\\x00")
file_names = st.text(name_chars, min_size=1, max_size=24).filter(
    lambda s: s not in (".", "..") and s.strip() == s and s
)
label_keys = st.text("abcdefghij", min_size=1, max_size=3)
label_vals = st.text("xyz012", min_size=1, max_size=3)
env_keys = st.from_regex(r"[A-Z_][A-Z0-9_]{0,10}", fullmatch=True)
env_vals = st.text(st.characters(blacklist_categories=("Cs", "Cc")), max_size=30)

_state: dict = {}


def client():
    from daytona import (  # noqa: F401
        CreateSandboxFromSnapshotParams,
        Daytona,
        DaytonaConfig,
    )

    if "d" not in _state:
        _state["d"] = Daytona(DaytonaConfig())
        _state["made"] = []
        _state["main"] = _state["d"].create(
            CreateSandboxFromSnapshotParams(labels={"props": "main"}, auto_stop_interval=0)
        )
        _state["made"].append(_state["main"])
    return _state["d"], _state["main"]


@SETTINGS
@given(data=st.binary(max_size=70_000), name=file_names)
def prop_file_roundtrip(data, name):
    _, sb = client()
    path = f"/tmp/props/{name}"
    sb.fs.upload_file(data, path)
    assert sb.fs.download_file(path) == data
    info = sb.fs.get_file_info(path)
    assert info.size == len(data) and info.name == name and not info.is_dir


@SETTINGS
@given(files=st.dictionaries(file_names, st.binary(max_size=2000), min_size=1, max_size=8))
def prop_bulk_roundtrip(files):
    from daytona import FileDownloadRequest, FileUpload

    _, sb = client()
    sb.fs.upload_files(
        [FileUpload(source=v, destination=f"/tmp/propsb/{k}") for k, v in files.items()]
    )
    got = sb.fs.download_files([FileDownloadRequest(source=f"/tmp/propsb/{k}") for k in files])
    assert [(r.source, r.result, r.error) for r in got] == [
        (f"/tmp/propsb/{k}", v, None) for k, v in files.items()
    ]


@SETTINGS
@given(
    env=st.dictionaries(env_keys, env_vals, max_size=6),
    labels=st.dictionaries(label_keys, label_vals, max_size=4),
)
def prop_create_keeps_env_and_labels(env, labels):
    from daytona import CreateSandboxFromSnapshotParams

    d, _ = client()
    sb = d.create(
        CreateSandboxFromSnapshotParams(env_vars=env, labels=labels, auto_stop_interval=0)
    )
    _state["made"].append(sb)
    try:
        got = d.get(sb.id)
        assert got.env == env
        assert {k: v for k, v in got.labels.items() if k != "code-toolbox-language"} == labels
    finally:
        d.delete(sb)
        _state["made"].remove(sb)


@SETTINGS
@given(
    sets=st.lists(st.dictionaries(label_keys, label_vals, max_size=3), min_size=1, max_size=4),
    query=st.dictionaries(label_keys, label_vals, max_size=2),
    limit=st.integers(1, 5),
)
def prop_list_matches_a_model(sets, query, limit):
    from daytona import CreateSandboxFromSnapshotParams, ListSandboxesQuery

    d, _ = client()
    tag = f"p{os.getpid()}{len(_state['made'])}{abs(hash(json.dumps(sets))) % 100000}"
    made = []
    try:
        for s in sets:
            sb = d.create(
                CreateSandboxFromSnapshotParams(labels={**s, "ptag": tag}, auto_stop_interval=0)
            )
            made.append((sb, s))
        want = {sb.id for sb, s in made if all(s.get(k) == v for k, v in query.items())}
        got = [x.id for x in d.list(ListSandboxesQuery(labels={**query, "ptag": tag}, limit=limit))]
        assert sorted(got) == sorted(want) and len(got) == len(set(got)), (got, want)
    finally:
        for sb, _ in made:
            d.delete(sb)


PROPS = [
    prop_file_roundtrip,
    prop_bulk_roundtrip,
    prop_create_keeps_env_and_labels,
    prop_list_matches_a_model,
]


def main():
    if "--env" in sys.argv:
        os.environ.update(json.load(open(sys.argv[sys.argv.index("--env") + 1])))
    failed = []
    for p in PROPS:
        try:
            p()
            print(f"PASS {p.__name__}")
        except BaseException as e:  # noqa: BLE001
            failed.append(p.__name__)
            print(f"FAIL {p.__name__}: {type(e).__name__}: {str(e)[:600]}")
    d = _state.get("d")
    for sb in list(_state.get("made", [])):
        try:
            d.delete(sb)
        except Exception:  # noqa: BLE001
            pass
    print(f"{len(PROPS) - len(failed)} properties held, {len(failed)} failed {failed}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
