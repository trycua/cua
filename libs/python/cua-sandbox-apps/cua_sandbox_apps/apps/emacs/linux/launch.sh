#!/usr/bin/env python3
import os
import subprocess
import time

os.environ['DISPLAY'] = ':1'

# Launch Emacs in background
proc = subprocess.Popen(['emacs'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
print(f"Emacs started with PID: {proc.pid}")

# Wait a moment for it to render
time.sleep(3)

print("Done")