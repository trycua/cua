#!/bin/bash
# Sync local dakar directory to remote VM
# Usage: ./sync-to-remote.sh [--watch]

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REMOTE_HOST="vmuser@172.190.205.135"
SSH_KEY="$SCRIPT_DIR/vmuser.pem"
LOCAL_DIR="$SCRIPT_DIR/"
REMOTE_DIR="~/cua-bench/"

sync_once() {
    rsync -avz --progress \
        -e "ssh -i $SSH_KEY" \
        --exclude '.git' \
        --exclude '__pycache__' \
        --exclude '*.pyc' \
        --exclude 'node_modules' \
        --exclude '.DS_Store' \
        --exclude 'sync-to-remote.sh' \
        "$LOCAL_DIR" \
        "$REMOTE_HOST:$REMOTE_DIR"
}

if [ "$1" == "--watch" ]; then
    echo "Watching for changes in $LOCAL_DIR..."
    echo "Press Ctrl+C to stop"

    # Initial sync
    sync_once

    # Watch for changes using fswatch (install with: brew install fswatch)
    fswatch -o "$LOCAL_DIR" --exclude '.git' --exclude '__pycache__' | while read f; do
        echo "Change detected, syncing..."
        sync_once
    done
else
    sync_once
fi
