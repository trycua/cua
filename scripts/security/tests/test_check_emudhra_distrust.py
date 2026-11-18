import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("chk", Path(__file__).parents[1] / "check-emudhra-distrust.py")
chk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(chk)


def lock(name, v):
    return f'[[package]]\nname = "{name}"\nversion = "{v}"\n'


class Guard(unittest.TestCase):
    def test_webpki(self):
        self.assertTrue(chk.check_cargo_lock(lock("rustls-webpki", "0.103.15")))
        self.assertTrue(chk.check_cargo_lock(lock("rustls-webpki", "0.103.13")))
        self.assertFalse(chk.check_cargo_lock(lock("rustls-webpki", "0.103.16")))
        self.assertFalse(chk.check_cargo_lock(lock("rustls-webpki", "0.104.0")))

    def test_webpki_roots_only_1x_is_gated(self):
        self.assertTrue(chk.check_cargo_lock(lock("webpki-roots", "1.0.9")))
        self.assertFalse(chk.check_cargo_lock(lock("webpki-roots", "1.0.10")))
        self.assertFalse(chk.check_cargo_lock(lock("webpki-roots", "0.26.11")))

    def test_certifi(self):
        self.assertTrue(chk.check_uv_lock(lock("certifi", "2026.7.22")))
        self.assertFalse(chk.check_uv_lock(lock("certifi", "2026.10.19")))
        self.assertTrue(chk.check_requirements("certifi==2025.1.1\n"))
        self.assertFalse(chk.check_requirements("certifi==2026.10.19\nrequests\n"))

    def test_electron(self):
        for v in ("39.8.5", "41.10.6", "42.11.15", "43.7.6", "44.5.1"):
            self.assertTrue(chk.check_package_json('{"dependencies":{"electron":"%s"}}' % v), v)
        for v in ("42.11.16", "^42.11.17", "43.8.1", "44.9.1", "45.0.0"):
            self.assertFalse(chk.check_package_json('{"devDependencies":{"electron":"%s"}}' % v), v)
        self.assertTrue(chk.check_pnpm_lock("  electron@44.5.1:\n    resolution\n"))
        self.assertFalse(chk.check_pnpm_lock("  electron@44.9.1(supports-color@7.2.0):\n"))

    def test_go(self):
        self.assertTrue(chk.check_go_mod("go 1.24.0\ntoolchain go1.24.7\n"))
        self.assertTrue(chk.check_go_mod("go 1.25.8\n"))
        self.assertFalse(chk.check_go_mod("go 1.25.0\ntoolchain go1.26.10\n"))
        self.assertTrue(chk.check_go_mod("go 1.27.1\n"))
        self.assertFalse(chk.check_go_mod("go 1.27.3\n"))

    def test_dockerfile(self):
        self.assertTrue(chk.check_dockerfile("FROM debian\nRUN apt-get install -y ca-certificates\n"))
        self.assertFalse(chk.check_dockerfile("FROM debian\nRUN apt-get install -y ca-certificates\nRUN sed emSign_ECC_Root_CA_-_G3\n"))
        self.assertFalse(chk.check_dockerfile("FROM debian\nRUN apt-get install -y curl\n"))


if __name__ == "__main__":
    unittest.main()
