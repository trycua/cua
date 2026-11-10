"""Cua Cloud has ended: ``--on cloud`` and ``pool:<name>`` images fail with one message."""

import pytest
from cua_bench._ended import CLOUD_ENDED
from cua_bench.targets import CloudEnded, Target, TargetError, resolve_env_spec, resolve_target

MESSAGE = (
    "Cua Cloud has ended; use a local sandbox or your own machine. "
    "See https://cua.ai/docs/cua-sdk/guides/cua-cloud-ended"
)


def test_the_message_is_exact():
    assert CLOUD_ENDED == MESSAGE
    assert str(CloudEnded()) == MESSAGE
    assert isinstance(CloudEnded(), TargetError)


def test_on_cloud_is_refused():
    with pytest.raises(CloudEnded, match="Cua Cloud has ended"):
        resolve_target("cloud")


def test_a_cloud_default_is_refused():
    with pytest.raises(CloudEnded):
        resolve_target(environ={"CUA_DEFAULT_ON": "cloud"})


def test_a_pool_image_is_refused():
    with pytest.raises(CloudEnded):
        resolve_env_spec({"provider": "native"}, Target(on="local", image="pool:bench-p"))


def test_local_still_resolves():
    assert resolve_target("local").on == "local"
