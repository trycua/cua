"""Smoke tests — verify models and TrainClient can be imported and instantiated."""

from cua_train import AuthenticatedClient, TrainClient
from cua_train.models import (
    Action,
    ActionParameters,
    BatchProgress,
    BatchSubmitRequest,
    LabelStatusResponse,
    RunConfig,
)


def test_batch_submit_request_instantiation():
    click = Action(action_type="CLICK", parameters=ActionParameters.from_dict({"x": 1, "y": 2}))
    req = BatchSubmitRequest(
        runs=[RunConfig(steps=[click], timeout=60)],
        concurrency=2,
    )
    assert len(req.runs) == 1
    assert req.runs[0].steps[0].action_type == "CLICK"


def test_label_status_response_instantiation():
    resp = LabelStatusResponse(
        label="test-label",
        batch_count=1,
        all_completed=False,
        progress=BatchProgress(completed=0, total=1, ok=0, failed=0),
        batches=[],
    )
    assert resp.label == "test-label"


def test_train_client_is_subclass():
    """TrainClient must be a drop-in for AuthenticatedClient."""
    assert issubclass(TrainClient, AuthenticatedClient)


def test_client_exposes_generated_operations():
    """Every generated endpoint is reachable as a method on the client
    (its sync(), client pre-bound), with no hand-written wrapper."""
    import pytest

    c = TrainClient(base_url="https://example.invalid", token="t")

    # A representative op from each tag is exposed and callable.
    for op in (
        "acquire_lanes",
        "release_lanes",
        "submit_batch",
        "submit_label_batch",
        "get_label_results",
        "list_keys",
        "healthz",
    ):
        assert callable(getattr(c, op)), f"client.{op} should be callable"

    # Unknown attributes still raise AttributeError (so copy/pickle/etc. behave).
    with pytest.raises(AttributeError):
        c.definitely_not_an_endpoint
