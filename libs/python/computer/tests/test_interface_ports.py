from computer.computer.interface.linux import LinuxComputerInterface


def test_linux_interface_default_port():
    interface = LinuxComputerInterface("127.0.0.1")
    assert interface.ws_uri == "ws://127.0.0.1:8000/ws"
    assert interface.rest_uri == "http://127.0.0.1:8000/cmd"


def test_linux_interface_custom_port():
    interface = LinuxComputerInterface("127.0.0.1", port=18000)
    assert interface.ws_uri == "ws://127.0.0.1:18000/ws"
    assert interface.rest_uri == "http://127.0.0.1:18000/cmd"


def test_linux_interface_secure_default_port():
    interface = LinuxComputerInterface("example.com", api_key="secret")
    assert interface.ws_uri == "wss://example.com:8443/ws"
    assert interface.rest_uri == "https://example.com:8443/cmd"


def test_linux_interface_secure_custom_port():
    interface = LinuxComputerInterface("example.com", api_key="secret", port=18000)
    assert interface.ws_uri == "wss://example.com:18000/ws"
    assert interface.rest_uri == "https://example.com:18000/cmd"
