"""Swift/Python parity for the MB-* derivations (needs xcrun swiftc; skipped when it is missing).

Compiles BenchLab with -D BENCHLAB_DUMP and compares `--dump-params` with benchlab_common for 60 seeds.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "_common"))
import benchlab_common as C  # noqa: E402

SWIFT = HERE.parent.parent / "swift"
SEEDS = [0, 1, 2, 3, 5, 7, 11, 42, 999, 123456, 999999] + list(range(1000, 1049))


class MbParity(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        if shutil.which("xcrun") is None:
            raise unittest.SkipTest("xcrun not available")
        cls.tmp = tempfile.mkdtemp(prefix="mbparity-")
        cls.exe = str(Path(cls.tmp) / "benchlab_dump")
        subprocess.run(
            [
                "xcrun",
                "swiftc",
                "-O",
                "-parse-as-library",
                "-swift-version",
                "5",
                "-target",
                "arm64-apple-macosx26.0",
                "-D",
                "BENCHLAB_DUMP",
                *[str(p) for p in sorted(SWIFT.glob("BenchLab*.swift"))],
                "-o",
                cls.exe,
            ],
            check=True,
            capture_output=True,
        )

    @classmethod
    def tearDownClass(cls) -> None:
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def dump(self, seed: int) -> dict:
        out = subprocess.run(
            [
                self.exe,
                "--dump-params",
                "--seed",
                str(seed),
                "--mode",
                "forms",
                "--state",
                "/dev/null",
                "--events",
                "/dev/null",
            ],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        return json.loads(out)

    def test_parity(self) -> None:
        for seed in SEEDS:
            sw = self.dump(seed)
            py = C.derive_mode_params(seed)
            # canvasmenu: targets + actions
            self.assertEqual(sw["canvasmenu"]["targets"], py["canvasmenu"]["targets"], seed)
            self.assertEqual(sw["canvasmenu"]["actions"], py["canvasmenu"]["actions"], seed)
            # tablesel
            for k in ("code", "row", "name", "qty", "decoys"):
                self.assertEqual(sw["tablesel"][k], py["tablesel"][k], (seed, k))
            # richtext
            for k in ("sentences", "bold_index", "replace_index", "old_word", "new_word"):
                self.assertEqual(sw["richtext"][k], py["richtext"][k], (seed, k))
            # settings
            for k in ("name", "theme", "compact_default", "compact_target", "ticket"):
                self.assertEqual(sw["settings"][k], py["settings"][k], (seed, k))
            # listdrag, tooltip
            self.assertEqual(sw["listdrag"], py["listdrag"], seed)
            for k in ("order", "target_index", "target", "codes"):
                self.assertEqual(sw["tooltip"][k], py["tooltip"][k], (seed, k))
            # Bench v2 interruption probes (IR-01..IR-04)
            ir = C.derive_ir_params(seed)
            for mode in ("irmodal", "irbanner", "irunsaved", "irconsent"):
                self.assertEqual(sw[mode], ir[mode], (seed, mode))


class DerivationShape(unittest.TestCase):
    def test_richtext_words_unique_and_ranges(self) -> None:
        for seed in range(2000):
            d = C.derive_richtext(seed)
            para = d["paragraph"]
            words = para.replace(".", "").split()
            self.assertEqual(words.count(d["old_word"]), 1, (seed, para))
            self.assertNotEqual(d["bold_index"], d["replace_index"])
            loc, ln = d["bold_range"]
            self.assertEqual(d["final_text"][loc : loc + ln], d["bold_sentence"])

    def test_tablesel_unique_pair(self) -> None:
        for seed in range(500):
            d = C.derive_tablesel(seed)
            rows = [d["row"]] + [x["row"] for x in d["decoys"]]
            self.assertEqual(len(set(rows)), 4, seed)
            for x in d["decoys"]:
                self.assertNotEqual((x["name"], x["qty"]), (d["name"], d["qty"]), seed)

    def test_listdrag_distance(self) -> None:
        for seed in range(500):
            d = C.derive_listdrag(seed)
            self.assertGreaterEqual(sum(1 for a, b in zip(d["initial"], d["target"]) if a != b), 4)
            self.assertEqual(sorted(d["initial"]), sorted(d["target"]))

    def test_irunsaved_targets_differ(self) -> None:
        for seed in range(500):
            d = C.derive_irunsaved(seed)
            self.assertEqual(len(set(d["titles"])), 3, seed)
            self.assertNotEqual(d["status_index"], d["rename_index"], seed)
            self.assertNotIn(d["new_title"], d["titles"], seed)
            self.assertNotEqual(d["status"], C.IR_INITIAL_STATUS, seed)

    def test_tooltip_codes_distinct(self) -> None:
        for seed in range(500):
            d = C.derive_tooltip(seed)
            self.assertEqual(len(set(d["codes"])), 4, seed)


if __name__ == "__main__":
    unittest.main()
