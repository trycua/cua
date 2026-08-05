"""Oracle runs must not report success when the reference solution scores nothing.

`cb interact --oracle` runs the task's own reference solution. A non-positive
score therefore means the task is broken, not that the operator underperformed,
so the command must not print "Task completed successfully!" and exit 0.
"""

from cua_bench.cli.commands.interact import oracle_validation_failed


def test_zero_score_is_a_failed_oracle():
    # The reported shape: the bundled 2048 example evaluated to [0.0] while the
    # CLI still announced success.
    assert oracle_validation_failed([0.0]) is True


def test_any_positive_score_passes():
    assert oracle_validation_failed([0.25]) is False
    assert oracle_validation_failed([1.0]) is False


def test_mixed_scores_pass_when_any_is_positive():
    # Multi-metric evaluators return one entry per metric; scoring on any of
    # them is evidence the oracle drove the environment.
    assert oracle_validation_failed([0.0, 0.5, 0.0]) is False


def test_all_zero_multi_metric_fails():
    assert oracle_validation_failed([0.0, 0.0]) is True


def test_negative_scores_fail():
    # Penalty-style evaluators can go below zero; that is not a solved task.
    assert oracle_validation_failed([-1.0]) is True


def test_bare_number_is_accepted():
    assert oracle_validation_failed(0.0) is True
    assert oracle_validation_failed(1.0) is False


def test_missing_or_empty_result_is_inconclusive_not_failed():
    # An evaluator that returns nothing gives no evidence either way. Failing
    # here would break tasks that report through a side channel.
    assert oracle_validation_failed(None) is False
    assert oracle_validation_failed([]) is False


def test_non_numeric_results_are_inconclusive():
    # Custom evaluator payloads must keep working rather than hard-failing.
    assert oracle_validation_failed("done") is False
    assert oracle_validation_failed({"score": 0.0}) is False
    assert oracle_validation_failed([object()]) is False


def test_booleans_are_not_treated_as_scores():
    # bool is a subclass of int; True must not silently count as a score of 1.0.
    assert oracle_validation_failed([True]) is False
    assert oracle_validation_failed(True) is False


def test_numeric_entries_are_read_past_non_numeric_ones():
    # A mixed payload still yields a verdict from the numbers present.
    assert oracle_validation_failed([None, 0.0, "x"]) is True
    assert oracle_validation_failed([None, 0.75, "x"]) is False
