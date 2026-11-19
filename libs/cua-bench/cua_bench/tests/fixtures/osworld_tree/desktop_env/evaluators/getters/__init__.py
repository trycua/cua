import json
import urllib.request


def get_vm_command_line(env, config):
    body = json.dumps({"command": config["command"], "shell": config.get("shell", False)}).encode()
    req = urllib.request.Request(f"http://{env.vm_ip}:{env.server_port}/execute", data=body,
                                 headers={"content-type": "application/json"})
    return json.loads(urllib.request.urlopen(req, timeout=10).read())["output"]


def get_rule(env, config):
    return config["rules"]
