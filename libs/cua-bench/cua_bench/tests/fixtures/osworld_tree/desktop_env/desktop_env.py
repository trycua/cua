# Fixture: the attribute contract of DesktopEnv._set_task_info/evaluate.
from desktop_env.evaluators import getters, metrics


class DesktopEnv:
    def _set_task_info(self, task_config):
        self.task_id = task_config["id"]
        self.cache_dir = self.cache_dir_base + "/" + self.task_id
        self.config = task_config.get("config", [])
        self.evaluator = task_config["evaluator"]
        self.metric = getattr(metrics, self.evaluator["func"])
        self.result_getter = getattr(getters, "get_" + self.evaluator["result"]["type"]) if self.evaluator.get("result") else None
        self.expected_getter = getattr(getters, "get_" + self.evaluator["expected"]["type"]) if self.evaluator.get("expected") else None

    def evaluate(self):
        self.setup_controller.setup(self.evaluator.get("postconfig", []), self.enable_proxy)
        if self.evaluator["func"] == "infeasible":
            return 1 if self.action_history and self.action_history[-1] == "FAIL" else 0
        if self.action_history and self.action_history[-1] == "FAIL":
            return 0
        result = self.result_getter(self, self.evaluator["result"])
        expected = self.expected_getter(self, self.evaluator["expected"])
        return self.metric(result, expected)
