# Fixture: the shape of OSWorld's PythonController (not upstream code).
import pyautogui  # always shimmed by the adapter (host safety)


class PythonController:
    def __init__(self, vm_ip, server_port, pkgs_prefix=""):
        self.vm_ip, self.server_port = vm_ip, server_port
        self.http_server = f"http://{vm_ip}:{server_port}"
