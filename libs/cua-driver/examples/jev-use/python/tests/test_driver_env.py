from __future__ import annotations

import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from driver_env import DESKTOP_SESSION_VARS, driver_environment


class DriverEnvironmentTest(unittest.TestCase):
    def test_forwards_desktop_session_variables_when_set(self):
        source = {name: f"value-{name}" for name in DESKTOP_SESSION_VARS}
        env = driver_environment(source)
        for name in DESKTOP_SESSION_VARS:
            self.assertEqual(env[name], f"value-{name}")

    def test_omits_desktop_session_variables_when_unset(self):
        with mock.patch.dict("os.environ", {"PATH": "/usr/bin"}, clear=True):
            env = driver_environment()
        for name in DESKTOP_SESSION_VARS:
            self.assertNotIn(name, env)

    def test_forwards_driver_variables(self):
        env = driver_environment({"CUA_DRIVER_PERMISSION_MODE": "unrestricted"})
        self.assertEqual(env["CUA_DRIVER_PERMISSION_MODE"], "unrestricted")

    def test_does_not_forward_provider_credentials(self):
        with mock.patch.dict(
            "os.environ",
            {"PATH": "/usr/bin", "DISPLAY": ":99", "TYPESAFE_API_KEY": "secret"},
            clear=True,
        ):
            env = driver_environment()
        self.assertEqual(env["DISPLAY"], ":99")
        self.assertEqual(env["PATH"], "/usr/bin")
        self.assertNotIn("TYPESAFE_API_KEY", env)
        self.assertNotIn("secret", env.values())


if __name__ == "__main__":
    unittest.main()
