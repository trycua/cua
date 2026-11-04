#!/usr/bin/env bash
# Healthy when the OSWorld server answers (it needs the X display) and
# cua-spacesd listens.
set -euo pipefail
python3 - <<'PY'
import socket, sys
for port in (5000, 3211):
    s = socket.socket(); s.settimeout(2)
    try:
        s.connect(("127.0.0.1", port))
    except OSError:
        sys.exit(1)
    finally:
        s.close()
PY
