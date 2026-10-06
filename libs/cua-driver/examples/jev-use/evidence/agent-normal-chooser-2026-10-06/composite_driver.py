"""Research adapter for the opt-in SDK action/read host; never caches a read."""
from __future__ import annotations
from typing import Any
from native import NativeObservation, NativeObservationError
from run import DriverToolError


def child_data(name: str, result: Any) -> dict[str, Any]:
    if not isinstance(result, dict) or not isinstance(result.get('content'), list):
        raise DriverToolError(f'{name}: malformed child result; observe effects before retrying')
    flag = result.get('isError', False)
    if not isinstance(flag, bool):
        raise DriverToolError(f'{name}: malformed error flag')
    data = result.get('structuredContent')
    if not isinstance(data, dict):
        raise DriverToolError(f'{name}: missing child data; observe effects before retrying')
    refusal = data.get('refusal')
    code = data.get('code') or (refusal.get('code') if isinstance(refusal, dict) else None)
    if flag or data.get('status') == 'refused' or refusal:
        raise DriverToolError(f'{name}: child error/refusal; input is not retried', code=code if isinstance(code, str) else None)
    return data


class CompositeTextDriver:
    """Delegate reads; pair each explicit set_value with a fresh same-owner read.

    This intentionally preserves the literal executor's preinput reobservations.
    The returned postaction read is validated and logged, never used as a cached
    next-step binding. A failed read terminates this call after its one input.
    """
    def __init__(self, driver: Any, pid: int, window_id: int):
        self.driver, self.pid, self.window_id = driver, pid, window_id
        self.receipts: list[dict[str, Any]] = []

    async def call(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        if name != 'set_value':
            return await self.driver.call(name, arguments)
        if arguments.get('pid') != self.pid or arguments.get('window_id') != self.window_id:
            raise DriverToolError('composite owner mismatch')
        action = dict(arguments)
        if hasattr(self.driver, 'label'):
            action['session'] = self.driver.label
        result = await self.driver.call('experiment_action_observe', {'tool': name, 'arguments': action})
        if not isinstance(result, dict):
            raise DriverToolError('malformed composite response; observe effects before retrying')
        acted = child_data(name, result.get('action_result'))
        if result.get('observation_available') is not True:
            raise DriverToolError('postaction observation unavailable; input is not retried')
        observed = child_data('get_window_state', result.get('observation_result'))
        try:
            parsed = NativeObservation.from_window_state(observed, expected_pid=self.pid, expected_window_id=self.window_id)
            if not parsed.snapshot_id or 'elements' not in observed:
                raise DriverToolError('postaction observation missing fresh snapshot/schema; input is not retried')
        except NativeObservationError as exc:
            raise DriverToolError('postaction observation owner/schema mismatch; input is not retried') from exc
        self.receipts.append({'action_result': acted, 'snapshot_id': observed.get('snapshot_id'), 'postaction_observed': True})
        return acted
