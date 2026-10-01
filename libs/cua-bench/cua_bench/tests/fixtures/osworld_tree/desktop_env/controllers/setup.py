# Fixture: the shape of OSWorld's SetupController (not upstream code).
import json
import urllib.request


class SetupController:
    def __init__(self, vm_ip, server_port=5000, chromium_port=9222, vlc_port=8080,
                 cache_dir="cache", client_password="", screen_width=1920, screen_height=1080):
        self.http_server = f"http://{vm_ip}:{server_port}"
        self.cache_dir = cache_dir
        self.client_password = client_password

    def reset_cache_dir(self, cache_dir):
        self.cache_dir = cache_dir

    def setup(self, config, use_proxy=False):
        for step in config:
            p = step["parameters"]
            body = json.dumps({"command": p["command"].replace("{CLIENT_PASSWORD}", self.client_password),
                               "shell": p.get("shell", False)}).encode()
            req = urllib.request.Request(self.http_server + "/setup/execute", data=body,
                                         headers={"content-type": "application/json"})
            urllib.request.urlopen(req, timeout=10).read()
        return True
