#!/bin/bash

# DBeaver Launch Script
# Launches DBeaver Community Edition

DBEAVER_BIN="$HOME/.local/opt/dbeaver/dbeaver"

if [ ! -f "$DBEAVER_BIN" ]; then
    echo "Error: DBeaver binary not found at $DBEAVER_BIN"
    exit 1
fi

# Launch DBeaver
exec "$DBEAVER_BIN" &