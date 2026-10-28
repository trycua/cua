"""``reward_of`` honours the documented Score contract.

``adapters.base`` documents a score as "a number, a bool, a list of numbers or
a dict (see runner.episode.reward_of)", and adapters routinely score with
numpy scalars (``np.bool_``, ``np.int64``, ``np.float32``) because their
getters compute over numpy arrays. ``reward_of`` must fold every one of those
to a scalar reward instead of silently returning ``None``.
"""

from __future__ import annotations

import pytest

from cua_bench.runner.episode import reward_of

np = pytest.importorskip("numpy")


class TestPythonScalars:
    """The pre-existing behaviour must not change."""

    def test_none_is_no_reward(self):
        assert reward_of(None) is None

    @pytest.mark.parametrize(
        ("value", "expected"),
        [(True, 1.0), (False, 0.0), (3, 3.0), (0.5, 0.5)],
    )
    def test_python_scalars(self, value, expected):
        assert reward_of(value) == expected

    def test_list_of_python_scalars_averages(self):
        assert reward_of([1.0, 0.0]) == 0.5

    def test_empty_list_is_no_reward(self):
        assert reward_of([]) is None

    def test_unknown_type_is_no_reward(self):
        assert reward_of("not a score") is None
        assert reward_of(object()) is None


class TestNumpyScalars:
    """numpy scalars are numbers; they must not collapse to ``None``."""

    @pytest.mark.parametrize(
        ("value", "expected"),
        [
            (np.bool_(True), 1.0),
            (np.bool_(False), 0.0),
            (np.int64(3), 3.0),
            (np.int32(3), 3.0),
            (np.uint8(2), 2.0),
            (np.float32(0.5), 0.5),
            (np.float64(0.5), 0.5),
        ],
    )
    def test_numpy_scalar(self, value, expected):
        assert reward_of(value) == expected

    def test_list_of_numpy_scalars_averages(self):
        assert reward_of([np.bool_(True), np.bool_(False)]) == 0.5

    def test_numpy_scalar_in_nested_list(self):
        assert reward_of([[np.int64(1)], [np.int64(3)]]) == 2.0


class TestDictScore:
    """A dict score carries its reward under the ``reward`` key."""

    def test_dict_reward_key(self):
        assert reward_of({"reward": 0.75}) == 0.75

    def test_dict_reward_numpy_scalar(self):
        assert reward_of({"reward": np.bool_(True)}) == 1.0

    def test_dict_reward_list_averages(self):
        assert reward_of({"reward": [1.0, 0.0]}) == 0.5

    def test_dict_without_reward_key_is_no_reward(self):
        assert reward_of({"score": 1.0}) is None
