#!/usr/bin/env python3
"""The client's own cleanup: terminate every sandbox it still holds."""

import glob
import json
import os

import modal

for p in glob.glob(os.path.join(os.environ["CHAOS_DIR"], "slots", "*.json")):
    try:
        modal.Sandbox.from_id(json.load(open(p))["id"]).terminate()
    except Exception as e:
        print("terminate failed:", e)
    os.unlink(p)
