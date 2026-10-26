#!/usr/bin/env python3
"""The client's own cleanup: delete every sandbox it still holds."""

import glob
import json
import os

from daytona import Daytona

dt = Daytona()
for p in glob.glob(os.path.join(os.environ["CHAOS_DIR"], "slots", "*.json")):
    try:
        dt.get(json.load(open(p))["id"]).delete()
    except Exception as e:
        print("delete failed:", e)
    os.unlink(p)
