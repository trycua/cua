import logging

logger = logging.getLogger("desktopenv.metric.windows_clock")

def get_check_if_timer_started(env, config: dict) -> str:
    return env.controller.get_vm_check_if_timer_started(config["hours"], config["minutes"], config["seconds"])

def get_check_if_world_clock_exists(env, config: dict) -> str:
    return env.controller.get_vm_check_if_world_clock_exists(config["city"], config["country"])