"""Cua Cloud has ended: the one message every cloud entry point raises."""

from cua_sandbox._sdk import Unsupported

CLOUD_ENDED = (
    "Cua Cloud has ended; use a local sandbox or your own machine. "
    "See https://cua.ai/docs/cua-sdk/guides/cua-cloud-ended"
)


class CloudEnded(Unsupported):
    """Raised wherever a caller still asks for Cua Cloud (``on="cloud"``,
    ``local=False``, ``pool=``, a ``cloud:`` ref). A ``NotImplementedError``,
    like the other unsupported operations."""

    def __init__(self) -> None:
        super().__init__(CLOUD_ENDED)


def cloud_ended() -> CloudEnded:
    """The exception to ``raise`` at a cloud entry point."""
    return CloudEnded()
