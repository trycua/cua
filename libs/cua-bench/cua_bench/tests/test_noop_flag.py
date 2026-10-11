"""``cb run --noop``: setup and evaluate with no solver (the eval-parity null baseline)."""

from types import SimpleNamespace

import pytest

from cua_bench.cli.commands.run import agent_options


def args(**kw):
    base = dict(agent=None, agent_import_path=None, oracle=False, noop=False, model=None, max_steps=None)
    base.update(kw)
    return SimpleNamespace(**base)


def test_noop_is_dump_mode():
    opts = agent_options(args(noop=True))
    assert opts.dump and not opts.oracle and opts.agent is None
    assert opts.label == "dump"


def test_default_is_still_the_oracle():
    opts = agent_options(args())
    assert opts.oracle and not opts.dump


@pytest.mark.parametrize("extra", [{"oracle": True}, {"agent": "cua-agent"}])
def test_noop_refuses_a_solver(extra):
    with pytest.raises(ValueError, match="--noop"):
        agent_options(args(noop=True, **extra))
