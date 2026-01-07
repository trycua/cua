import logging
from typing import Dict
import requests
import os

logger = logging.getLogger("desktopenv.getters.general")


def get_vm_command_line(env, config: Dict[str, str]):
    vm_ip = env.vm_ip
    port = 5000

    command = config["command"]
    shell = config.get("shell", False)
    # shell = True

    logger.info(f"COMMAND: {command}")
    logger.info(f"SHELL: {shell}")
    response = requests.post(f"http://{vm_ip}:{port}/execute", json={"command": command, "shell": shell})
    # logger.info(f"VM CMD LINE: {response.json}")
    # response = requests.post("/execute", json={"command": command, "shell": shell})
    if response.status_code == 200:
        return response.json()["output"]
        # logger.info(f"CMD and SHELL: {command, shell}")
        # logger.info(f"RESPONSE succ: {response}")
        # return response.json()
    else:
        logger.error("Failed to get vm command line. Status code: %d", response.status_code)
        return None

def get_vm_command_error(env, config: Dict[str, str]):
    vm_ip = env.vm_ip
    port = 5000
    command = config["command"]
    shell = config.get("shell", False)

    response = requests.post(f"http://{vm_ip}:{port}/execute", json={"command": command, "shell": shell})

    print(response.json())

    if response.status_code == 200:
        return response.json()["error"]
    else:
        logger.error("Failed to get vm command line error. Status code: %d", response.status_code)
        return None


def get_vm_terminal_output(env, config: Dict[str, str]):
    return env.controller.get_terminal_output()
