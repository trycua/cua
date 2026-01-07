#!/bin/bash
# Sync local dakar directory to remote VM
# Usage: ./sync-to-remote.sh [--watch]

REMOTE_HOST="vmuser@172.190.205.135"
SSH_KEY="/Users/francescobonacci/conductor/workspaces/benchmark/denpasar-v1/vmuser.pem"
LOCAL_DIR="/Users/francescobonacci/conductor/workspaces/benchmark/denpasar-v1/benchmark/"
REMOTE_DIR="~/benchmark/"

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
