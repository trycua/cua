"""Synthetic oracle tests only; no plugin load, desktop, or input execution."""
import socket
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

from input_retirement_live import preflight
from retirement_evidence import active_experiment, bound_seats, replacement_refusal, retired_seats, service_owns_socket, socket_closed


def active():
    return {"name": "cua-hyprland-plugin", "state": "input_experiment", "configured": True, "abi": {"match": True},
            "transport": {"ready": True}, "experiment": {"transport_ready": True,
            "test_only": True, "protocol": 0,
            "seat_lifetime": "compositor", "upgrade": "desktop_restart", "lanes": [
                {"lane": lane, "held_button": 0, "held_keys": 0, "lease_active": False,
                 "drag_active": False, "reserved": False} for lane in (0, 1)]}}


def refusal():
    return {"name": "cua-hyprland-plugin", "configured": True, "state": "discovery_only",
            "compositor_epoch": 0, "abi": {"match": True}, "capabilities": {"enabled": []},
            "transport": {"ready": False, "last_error":
                "input seat lifetime unavailable; restart the desktop before loading a replacement plugin"}}


BOUND = '''[100] -> wl_registry#2.bind(41, "wl_seat", 7, new id [unknown]#10)
[101] wl_seat#10.capabilities(3)
[102] wl_seat#10.name("Cua-Test-Agent")
[103] -> wl_registry#2.bind(42, "wl_seat", 7, new id wl_seat#11)
[104] wl_seat#11.capabilities(3)
[105] wl_seat#11.name("Cua-Test-Agent-2")
'''
RETIRED = '''[200] wl_seat#10.capabilities(0)
[201] wl_registry#2.global_remove(41)
[202] wl_seat#11.capabilities(0)
[203] wl_registry#2.global_remove(42)
'''


class RetirementEvidenceTest(unittest.TestCase):
    def test_active_requires_two_idle_restart_required_lanes(self):
        active_experiment(active())
        for field, value in (("held_button", 272), ("held_keys", 1), ("lease_active", True),
                             ("drag_active", True), ("reserved", True)):
            candidate = active()
            candidate["experiment"]["lanes"][1][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                active_experiment(candidate)

    def test_missing_duplicate_and_wrong_contract_lanes_fail(self):
        for change in ({"lanes": []}, {"lanes": [active()["experiment"]["lanes"][0]] * 2},
                       {"upgrade": "hot_reload"}, {"seat_lifetime": "plugin"}, {"transport_ready": False}):
            candidate = active()
            candidate["experiment"].update(change)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                active_experiment(candidate)

    def test_structured_runtime_refusal(self):
        self.assertEqual(replacement_refusal(refusal()),
                         {"runtime_restart_refusal": True, "transport_ready": False})
        candidate = refusal()
        candidate["transport"]["last_error"] = "restart desktop before replacing input seats"
        replacement_refusal(candidate)

    def test_plugin_absence_disabled_configuration_and_abi_refusal_are_not_retirement(self):
        for change in ({"name": "other"}, {"configured": False}, {"abi": {"match": False}},
                       {"state": "input_experiment"}, {"compositor_epoch": 1},
                       {"experiment": {}}, {"capabilities": {"enabled": ["discovery"]}},
                       {"transport": {"ready": False, "last_error": "ABI mismatch"}},
                       {"transport": {"ready": False, "last_error": ""}}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                replacement_refusal({**refusal(), **change})
        for absent in ({}, {"error": "unknown request"}, {"ok": False}):
            with self.subTest(absent=absent), self.assertRaises(AssertionError):
                replacement_refusal(absent)

    def test_partial_status_never_passes(self):
        for field in refusal():
            candidate = refusal()
            del candidate[field]
            with self.subTest(field=field), self.assertRaises(AssertionError):
                replacement_refusal(candidate)

    def test_ready_transport_with_restart_error_still_fails(self):
        candidate = refusal()
        candidate["transport"]["ready"] = True
        with self.assertRaises(AssertionError):
            replacement_refusal(candidate)

    def test_both_wayland_log_resource_delimiters(self):
        for delimiter in ("#", "@"):
            with self.subTest(delimiter=delimiter):
                seats = bound_seats(BOUND.replace("#", delimiter))
                self.assertEqual(seats, {"Cua-Test-Agent": (10, 41), "Cua-Test-Agent-2": (11, 42)})
                self.assertEqual(retired_seats(seats, RETIRED.replace("#", delimiter)),
                                 {"retired_seats": 2, "capabilities_zero": True, "globals_removed": True})

    def test_absent_unbound_ambiguous_or_disabled_initial_seats_fail(self):
        for wire in ("", BOUND.replace('"Cua-Test-Agent-2"', '"primary"'),
                     BOUND.replace('new id wl_seat#11', 'new id wl_seat#12'),
                     BOUND.replace("capabilities(3)", "capabilities(0)"),
                     BOUND + 'wl_seat#13.name("Cua-Test-Agent")\n'):
            with self.subTest(wire=wire), self.assertRaises(AssertionError):
                bound_seats(wire)

    def test_each_resource_requires_both_retirement_events(self):
        lines = RETIRED.splitlines(keepends=True)
        for index in range(len(lines)):
            with self.subTest(index=index), self.assertRaises(AssertionError):
                retired_seats(bound_seats(BOUND), "".join(lines[:index] + lines[index + 1:]))

    def test_empty_or_duplicate_bindings_cannot_pass_retirement(self):
        for seats in ({}, {"Cua-Test-Agent": (10, 41)},
                      {"Cua-Test-Agent": (10, 41), "Cua-Test-Agent-2": (10, 41)}):
            with self.subTest(seats=seats), self.assertRaises(AssertionError):
                retired_seats(seats, RETIRED)

    def test_wrong_resource_ids_do_not_prove_cleanup(self):
        for wire in (RETIRED.replace("#10.", "#110."), RETIRED.replace("(41)", "(141)")):
            with self.subTest(wire=wire), self.assertRaises(AssertionError):
                retired_seats(bound_seats(BOUND), wire)

    def test_republished_global_or_reenabled_capabilities_fail(self):
        for suffix in ('wl_seat#10.capabilities(3)', 'wl_registry#2.global(43, "wl_seat", 7)'):
            with self.subTest(suffix=suffix), self.assertRaises(AssertionError):
                retired_seats(bound_seats(BOUND), RETIRED + suffix)

    def test_closure_requires_eof_or_reset(self):
        client = Mock()
        client.recv.return_value = b""
        self.assertTrue(socket_closed(client))
        client.recv.side_effect = ConnectionResetError()
        self.assertTrue(socket_closed(client))
        for error in (socket.timeout(), PermissionError(), OSError()):
            client.recv.side_effect = error
            with self.subTest(error=type(error)), self.assertRaises(type(error)):
                socket_closed(client)

    def test_open_connection_or_buffered_response_is_not_closure(self):
        client = Mock()
        client.recv.return_value = b'{"ok":true}'
        with self.assertRaises(AssertionError):
            socket_closed(client)

    def test_opt_in_refuses_before_any_hyprctl_query(self):
        with patch("input_retirement_live.ctl") as command, patch("input_retirement_live.sys.platform", "linux"):
            with self.assertRaisesRegex(ValueError, "opt-in"):
                preflight(SimpleNamespace(disposable_session_restart_required=False))
            command.assert_not_called()

    def test_non_linux_refuses_even_with_opt_in(self):
        with patch("input_retirement_live.ctl") as command, patch("input_retirement_live.sys.platform", "darwin"):
            with self.assertRaises(ValueError):
                preflight(SimpleNamespace(disposable_session_restart_required=True))
            command.assert_not_called()

    def test_wrong_selected_instance_refuses_before_status_or_mutation(self):
        args = SimpleNamespace(disposable_session_restart_required=True, compositor_pid=123)
        with patch("input_retirement_live.sys.platform", "linux"), patch.dict(
                "input_retirement_live.os.environ", {"HYPRLAND_INSTANCE_SIGNATURE": "synthetic-instance"}), patch(
                "input_retirement_live.ctl", return_value='[{"instance":"synthetic-instance","pid":456}]') as command:
            with self.assertRaisesRegex(ValueError, "selected disposable compositor"):
                preflight(args)
            command.assert_called_once_with("-j", "instances")

    def test_service_socket_requires_matching_inode_and_exact_path(self):
        table = "000: 00000002 00000000 00010000 0001 01 456 /run/synthetic/driver.sock\n"
        service_owns_socket(table, ["socket:[456]"], "/run/synthetic/driver.sock")
        for records, links, path in ((table, ["socket:[789]"], "/run/synthetic/driver.sock"),
                                     (table, ["socket:[456]"], "/run/other/driver.sock"),
                                     (table + table, ["socket:[456]"], "/run/synthetic/driver.sock"),
                                     ("", ["socket:[456]"], "/run/synthetic/driver.sock")):
            with self.subTest(records=records, links=links, path=path), self.assertRaises(AssertionError):
                service_owns_socket(records, links, path)


if __name__ == "__main__":
    unittest.main()
