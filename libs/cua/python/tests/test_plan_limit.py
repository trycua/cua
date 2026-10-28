"""The relay's plan-limit refusal reads back as a typed `PlanLimitError`."""

import cua

SENTENCE = "You're using 5 of 5 live sessions on Pro. Close one or upgrade to Teams."


def test_a_sessions_refusal_carries_its_numbers_and_the_upgrade_link():
    error = cua.CuaError.Http(f"{SENTENCE} Upgrade: https://app.cua.ai/buy/teams?from=plan_limit")
    refusal = error.plan_limit
    assert isinstance(refusal, cua.PlanLimitError)
    assert isinstance(refusal, cua.CuaError.Http)
    assert str(refusal) == SENTENCE
    assert refusal.code == "plan_limit"
    assert (refusal.kind, refusal.plan, refusal.used, refusal.limit) == ("sessions", "pro", 5, 5)
    assert refusal.upgrade_url == "https://app.cua.ai/buy/teams?from=plan_limit"
    assert refusal.status == 429


def test_remote_hours_on_the_free_plan_without_a_link():
    text = "You've used 20.5 of 20 remote hours this month on the free plan. Upgrade to Pro for more."
    refusal = cua.PlanLimitError.from_error(cua.CuaError.Http("relay: " + text))
    assert refusal is not None
    assert str(refusal) == text
    assert (refusal.kind, refusal.plan, refusal.used, refusal.limit) == ("hours", "free", 20.5, 20)
    assert refusal.upgrade_url is None


def test_any_other_error_is_not_a_plan_limit():
    assert cua.CuaError.Http("http 500").plan_limit is None
    assert cua.PlanLimitError.from_error(cua.CuaError.NotFound("x")) is None
