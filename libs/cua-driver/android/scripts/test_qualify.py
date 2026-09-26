#!/usr/bin/env python3
"""Focused stdlib tests for qualify.py decisions; no Android device required."""
import importlib.util
import pathlib
import unittest


spec = importlib.util.spec_from_file_location("qualify", pathlib.Path(__file__).with_name("qualify.py"))
qualify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qualify)

EMULATOR = {"ro.kernel.qemu": "1", "ro.build.version.sdk": "37"}
PHONE = {"ro.kernel.qemu": "", "ro.boot.qemu": "", "ro.build.version.sdk": "37"}
ONLINE = {"emulator-5554": "device", "PHONE1": "device", "PHONE2": "unauthorized"}


class QualifyTest(unittest.TestCase):
    def test_each_role_refuses_the_other_device_class_before_mutation(self):
        self.assertEqual(qualify.admission("emulator", "emulator-5554", EMULATOR, ONLINE), [])
        self.assertEqual(qualify.admission("physical", "PHONE1", PHONE, ONLINE), [])
        self.assertEqual(qualify.admission("emulator", "PHONE1", PHONE, ONLINE), ["device_class_is_physical"])
        self.assertEqual(qualify.admission("physical", "emulator-5554", EMULATOR, ONLINE), ["device_class_is_emulator"])
        self.assertEqual(qualify.device_class({"ro.boot.qemu": "1"}), "emulator")

    def test_offline_or_unauthorized_devices_are_refused(self):
        self.assertEqual(qualify.admission("physical", "PHONE2", {}, ONLINE), ["not_online:unauthorized"])
        self.assertEqual(qualify.admission("physical", "MISSING", {}, ONLINE), ["not_online:absent"])

    def test_unsupported_api_levels_are_recorded_not_skipped(self):
        self.assertEqual(qualify.unsupported(PHONE), [])
        self.assertEqual(qualify.unsupported({"ro.build.version.sdk": "36"}), ["api_level_36_runtime_requires_37"])

    def test_physical_phase_runs_only_after_the_emulator_passes(self):
        self.assertIsNone(qualify.physical_gate("pass"))
        for status in ("fail", "unsupported"):
            self.assertEqual(qualify.physical_gate(status), "emulator_phase_" + status)

    def test_receipt_text_names_devices_by_role_only(self):
        text = "adb: device 'PHONE1' not found; emulator-5554 ok"
        redacted = qualify.redact(text, {"emulator": "emulator-5554", "physical": "PHONE1"})
        self.assertEqual(redacted, "adb: device '<physical>' not found; <emulator> ok")

    def test_cleanup_finds_synthetic_tasks_left_off_display_0(self):
        stack = """RootTask id=366 bounds=[0,0][2076,2152] displayId=0 userId=0
  taskId=366: ai.cua.android.demo/ai.cua.android.demo.MainActivity bounds=[0,0][2076,2152] userId=0
RootTask id=370 bounds=[0,0][1080,1920] displayId=48 userId=0
  taskId=370: ai.cua.fixture.notes/ai.cua.fixture.notes.MainActivity bounds=[0,0][1080,1920] userId=0
RootTask id=371 bounds=[0,0][1080,1920] displayId=49 userId=0
  taskId=371: com.example.other/com.example.other.Main bounds=[0,0][1080,1920] userId=0
"""
        self.assertEqual(qualify.synthetic_tasks_off_main_display(stack), [370])
        self.assertEqual(qualify.synthetic_tasks_off_main_display(stack.split("RootTask id=370")[0]), [])


if __name__ == "__main__":
    unittest.main()
