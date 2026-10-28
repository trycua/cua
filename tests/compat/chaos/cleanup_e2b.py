#!/usr/bin/env python3
"""The client's own cleanup at the end of a run: kill every sandbox it still holds."""

import glob
import json
import os

from e2b import Sandbox

for p in glob.glob(os.path.join(os.environ["CHAOS_DIR"], "slots", "*.json")):
    try:
        Sandbox.kill(json.load(open(p))["id"])
    except Exception as e:
        print("kill failed:", e)
    os.unlink(p)
