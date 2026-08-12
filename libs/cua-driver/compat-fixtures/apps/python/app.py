"""Frozen cua-driver-rs-v0.12.6 application source."""

from cua_driver import CuaDriver


driver = CuaDriver.connect(None)
endpoint = driver.socket_path()
if not endpoint.strip():
    raise RuntimeError("default endpoint must be selected")
print(endpoint)
