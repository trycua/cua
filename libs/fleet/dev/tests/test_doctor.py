import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "doctor", Path(__file__).parents[1] / "scripts" / "doctor.py")
doctor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(doctor)


class RoutesTest(unittest.TestCase):
    def test_default_and_unrelated_routes(self):
        self.assertEqual(doctor.overlaps([
            {"dst": "default"}, {"dst": "10.0.2.0/24"}, {"dst": "::1/128"}
        ]), [])

    def test_exact_overlap(self):
        self.assertEqual(len(doctor.overlaps([{"dst": "10.211.0.0/16"}])), 1)

    def test_supernet_overlap(self):
        self.assertEqual(len(doctor.overlaps([{"dst": "10.0.0.0/8"}])), 3)

    def test_host_route_overlap(self):
        self.assertEqual(len(doctor.overlaps([{"dst": "10.210.0.13/32"}])), 1)


class CoverageTest(unittest.TestCase):
    def test_legacy_list_does_not_authorize_network(self):
        routes, missing = doctor.outer_evidence([])
        self.assertEqual(routes, [])
        self.assertEqual(missing, list(doctor.REQUIRED_COVERAGE))

    def test_partial_inventory_stays_blocked(self):
        _, missing = doctor.outer_evidence({
            "routes": [], "coverage": {"node_pod_cidrs": True, "outer_routes": True}})
        self.assertEqual(missing, ["service_cidrs", "vpn_routes"])

    def test_complete_coverage(self):
        _, missing = doctor.outer_evidence({
            "routes": [], "coverage": dict.fromkeys(doctor.REQUIRED_COVERAGE, True)})
        self.assertEqual(missing, [])

    def test_truthy_strings_are_not_verified_coverage(self):
        _, missing = doctor.outer_evidence({
            "routes": [], "coverage": dict.fromkeys(doctor.REQUIRED_COVERAGE, "false")})
        self.assertEqual(missing, list(doctor.REQUIRED_COVERAGE))


if __name__ == "__main__":
    unittest.main()
