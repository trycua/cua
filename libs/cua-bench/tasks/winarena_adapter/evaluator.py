"""Evaluator dispatcher for WAA tasks.

Ties together getters (result/expected extraction) and metrics (comparison functions)
to produce evaluation scores.

This evaluator uses async getters that communicate with the VM via session.* methods
(HTTP calls to cua-computer-server), allowing evaluation to run in worker containers
rather than inside the VM.
"""

import logging
from typing import Any, Callable, Dict, List, Optional, Union

# Import evaluator modules - async getters and metrics
from .evaluators import metrics
from .evaluators.getters_async import get_async_getter
from .setup_controller import WAASetupController

logger = logging.getLogger("winarena.evaluator")


class SessionInterfaceWrapper:
    """Wrapper that provides a clean interface for async getters.

    Async getters expect methods like session.run_command(), session.read_bytes(), etc.
    This wrapper directly uses the session's methods (which call cua-computer-server via HTTP).
    """

    def __init__(self, session):
        """Initialize wrapper with cua-bench session.

        Args:
            session: cua-bench DesktopSession (e.g., RemoteDesktopSession)
        """
        self._session = session

    async def run_command(self, command: str):
        """Execute a shell command on the VM."""
        return await self._session.run_command(command)

    async def read_bytes(self, path: str) -> Optional[bytes]:
        """Read a file as bytes from the VM."""
        try:
            return await self._session.read_bytes(path)
        except Exception:
            return None

    async def read_text(self, path: str) -> Optional[str]:
        """Read a file as text from the VM."""
        try:
            return await self._session.read_file(path)
        except Exception:
            return None

    async def file_exists(self, path: str) -> bool:
        """Check if a file exists on the VM."""
        try:
            return await self._session.file_exists(path)
        except Exception:
            return False

    async def directory_exists(self, path: str) -> bool:
        """Check if a directory exists on the VM."""
        try:
            return await self._session.directory_exists(path)
        except Exception:
            return False

    async def list_dir(self, path: str) -> Optional[List[str]]:
        """List contents of a directory on the VM."""
        try:
            return await self._session.list_dir(path)
        except Exception:
            return None

    async def get_accessibility_tree(self) -> Optional[Dict[str, Any]]:
        """Get the accessibility tree from the VM."""
        try:
            result = await self._session.get_accessibility_tree()
            if isinstance(result, dict) and "tree" in result:
                return result.get("tree")
            return result
        except Exception:
            return None

    async def get_screen_size(self) -> Optional[Dict[str, int]]:
        """Get the VM screen size."""
        # Return session's configured dimensions
        return {
            "width": getattr(self._session, "_width", 1920),
            "height": getattr(self._session, "_height", 1080),
        }

    async def get_window_size(self, window_id) -> Optional[Dict[str, int]]:
        """Get the size of a window - not directly supported."""
        return None

    async def get_application_windows(self, app_name: str) -> Optional[List]:
        """Get windows for an application - not directly supported."""
        return None


class WAAEvaluator:
    """Dispatch WAA evaluation using async getters and metrics.

    This evaluator runs in the worker container and communicates with the VM
    via session.* methods (HTTP calls to cua-computer-server).

    Follows the WAA evaluation pattern:
    1. Run postconfig (setup operations before evaluation)
    2. Get result using async getter function
    3. Get expected using async getter function
    4. Call metric function with result and expected
    5. Return score
    """

    def __init__(self, session, cache_dir: str = "/tmp/waa_cache"):
        """Initialize evaluator.

        Args:
            session: cua-bench DesktopSession with VM communication methods
            cache_dir: Directory for caching files locally in worker
        """
        self._raw_session = session
        self.session = SessionInterfaceWrapper(session)
        self.cache_dir = cache_dir

    async def evaluate(self, evaluator_config: Dict[str, Any]) -> float:
        """Execute evaluation and return score.

        Args:
            evaluator_config: Evaluator configuration from task JSON:
                - func: Metric function name(s)
                - result: Result getter config(s)
                - expected: Expected getter config(s)
                - options: Metric options
                - postconfig: Setup operations before evaluation
                - conj: Conjunction for multiple metrics ("and"/"or")

        Returns:
            Score between 0.0 and 1.0
        """
        # 1. Run postconfig if present
        postconfig = evaluator_config.get("postconfig", [])
        if postconfig:
            setup_ctrl = WAASetupController(self._raw_session, self.cache_dir)
            await setup_ctrl.setup(postconfig)

        # 2. Handle special "infeasible" case
        func = evaluator_config.get("func")
        if func == "infeasible":
            # Task is marked as infeasible - would need action history to evaluate
            logger.warning("Infeasible task - returning 0.0 (no action history available)")
            return 0.0

        # 3. Parse metric function(s)
        if isinstance(func, list):
            metric_funcs = [self._get_metric(f) for f in func]
        else:
            metric_funcs = self._get_metric(func)

        # 4. Parse getter(s)
        result_config = evaluator_config.get("result", {})
        expected_config = evaluator_config.get("expected", {})
        options = evaluator_config.get("options", {})
        conj = evaluator_config.get("conj", "and")

        # 5. Evaluate
        try:
            if isinstance(metric_funcs, list):
                return await self._evaluate_multiple(
                    metric_funcs,
                    result_config,
                    expected_config,
                    options,
                    conj,
                )
            else:
                return await self._evaluate_single(
                    metric_funcs,
                    result_config,
                    expected_config,
                    options,
                )
        except Exception as e:
            logger.error(f"Evaluation error: {e}")
            return 0.0

    def _get_metric(self, func_name: str) -> Callable:
        """Get metric function by name."""
        if hasattr(metrics, func_name):
            return getattr(metrics, func_name)
        else:
            logger.warning(f"Unknown metric function: {func_name}")
            return lambda *args, **kwargs: 0.0

    def _get_getter(self, getter_type: str) -> Optional[Callable]:
        """Get async getter function by type.

        Args:
            getter_type: The getter type name (e.g., 'vm_file', 'enable_do_not_track')

        Returns:
            Async getter function, or None if not found
        """
        getter = get_async_getter(getter_type)
        if getter is None:
            logger.warning(f"Unknown async getter: {getter_type}")
        return getter

    async def _get_result(self, config: Dict[str, Any]) -> Any:
        """Get result value using async getter.

        Args:
            config: Getter configuration with 'type' and getter-specific params

        Returns:
            Result value from the getter
        """
        if not config:
            return None

        getter_type = config.get("type")
        if not getter_type:
            return None

        getter = self._get_getter(getter_type)
        if getter is None:
            return None

        try:
            # Async getters take (session, config) and use session.* for VM calls
            result = await getter(self.session, config)
            return result
        except Exception as e:
            logger.error(f"Error getting result ({getter_type}): {e}")
            raise

    async def _get_expected(self, config: Dict[str, Any]) -> Any:
        """Get expected value using async getter.

        Args:
            config: Getter configuration with 'type' and getter-specific params

        Returns:
            Expected value from the getter, or inline rules for 'rule' type
        """
        if not config:
            return None

        expected_type = config.get("type")

        # Handle "rule" type specially - it's inline data, not a getter
        if expected_type == "rule":
            return config.get("rules", {})

        if not expected_type:
            return None

        getter = self._get_getter(expected_type)
        if getter is None:
            return None

        try:
            # Async getters take (session, config) and use session.* for VM calls
            result = await getter(self.session, config)
            return result
        except Exception as e:
            logger.error(f"Error getting expected ({expected_type}): {e}")
            return None

    async def _evaluate_single(
        self,
        metric_func: Callable,
        result_config: Dict[str, Any],
        expected_config: Dict[str, Any],
        options: Dict[str, Any],
    ) -> float:
        """Evaluate a single metric."""
        try:
            result_state = await self._get_result(result_config)
        except FileNotFoundError:
            logger.error("Result file not found")
            return 0.0
        except Exception as e:
            logger.error(f"Error getting result: {e}")
            return 0.0

        expected_state = await self._get_expected(expected_config)

        # Call metric
        try:
            if expected_state is not None:
                score = metric_func(result_state, expected_state, **options)
            else:
                score = metric_func(result_state, **options)

            # Normalize score
            if isinstance(score, (float, int, bool)):
                return float(score)
            else:
                logger.error(f"Invalid metric return type: {type(score)}")
                return 0.0

        except Exception as e:
            logger.error(f"Metric evaluation error: {e}")
            return 0.0

    async def _evaluate_multiple(
        self,
        metric_funcs: List[Callable],
        result_configs: Union[List[Dict], Dict],
        expected_configs: Union[List[Dict], Dict],
        options_list: Union[List[Dict], Dict],
        conj: str,
    ) -> float:
        """Evaluate multiple metrics with conjunction."""
        # Normalize to lists
        if not isinstance(result_configs, list):
            result_configs = [result_configs] * len(metric_funcs)
        if not isinstance(expected_configs, list):
            expected_configs = [expected_configs] * len(metric_funcs)
        if not isinstance(options_list, list):
            options_list = [options_list] * len(metric_funcs)

        results = []

        for idx, metric_func in enumerate(metric_funcs):
            result_config = result_configs[idx] if idx < len(result_configs) else {}
            expected_config = expected_configs[idx] if idx < len(expected_configs) else {}
            options = options_list[idx] if idx < len(options_list) else {}

            # Handle None configs
            if result_config is None:
                result_config = {}
            if expected_config is None:
                expected_config = {}
            if options is None:
                options = {}

            try:
                result_state = await self._get_result(result_config)
            except FileNotFoundError:
                logger.error(f"Result file not found for metric {idx}")
                if conj == "and":
                    return 0.0
                continue

            expected_state = await self._get_expected(expected_config)

            try:
                if expected_state is not None:
                    score = metric_func(result_state, expected_state, **options)
                else:
                    score = metric_func(result_state, **options)

                score = float(score) if isinstance(score, (int, float, bool)) else 0.0

            except Exception as e:
                logger.error(f"Metric {idx} error: {e}")
                score = 0.0

            # Early termination for conjunctions
            if conj == "and" and score == 0.0:
                return 0.0
            elif conj == "or" and score == 1.0:
                return 1.0

            results.append(score)

        if not results:
            return 0.0

        # Combine results
        if conj == "and":
            return sum(results) / len(results)
        else:  # "or"
            return max(results)
