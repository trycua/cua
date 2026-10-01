#!/bin/bash

# Arguments passed from execute_remote_script in vm.sh
# $1: VNC_PASSWORD
# $2: HOST_SHARED_PATH (Path inside VM where host shared dir is mounted, e.g., /Volumes/My Shared Files)

VNC_PASSWORD="$1"
# IMPORTANT: In the VM, the shared folder is always mounted at this fixed location
HOST_SHARED_PATH="/Volumes/My Shared Files"

# Set default value for VNC_DEBUG if not provided
VNC_DEBUG=${VNC_DEBUG:-0}

# Define the path to the user's optional on-logon script within the shared folder
USER_ON_LOGON_SCRIPT_PATH="$HOST_SHARED_PATH/lifecycle/on-logon.sh"

# Show basic information when debug is enabled
if [ "$VNC_DEBUG" = "1" ]; then
    echo "[VM] Lumier lifecycle script starting"
    echo "[VM] Looking for user script: $USER_ON_LOGON_SCRIPT_PATH"
fi

# Check if the user-provided script exists
if [ -f "$USER_ON_LOGON_SCRIPT_PATH" ]; then
    if [ "$VNC_DEBUG" = "1" ]; then
        echo "[VM] Found user script: $USER_ON_LOGON_SCRIPT_PATH"
    fi
    
    # Always show what script we're executing
    echo "[VM] Executing user lifecycle script"
    
    # Make script executable
    chmod +x "$USER_ON_LOGON_SCRIPT_PATH"
    
    # Execute the user script in a subshell with error output captured
    "$USER_ON_LOGON_SCRIPT_PATH" "$VNC_PASSWORD" "$HOST_SHARED_PATH" 2>&1
    
    # Capture exit code
    USER_SCRIPT_EXIT_CODE=$?
    
    # Always report script execution results
    if [ $USER_SCRIPT_EXIT_CODE -eq 0 ]; then
        echo "[VM] User lifecycle script completed successfully"
    else
        echo "[VM] User lifecycle script failed with exit code: $USER_SCRIPT_EXIT_CODE"
    fi
    
    # Check results (only in debug mode)
    if [ "$VNC_DEBUG" = "1" ]; then
        # List any files created by the script
        echo "[VM] Files created by user script:"
        ls -la /Users/lume/Desktop/hello_*.txt 2>/dev/null || echo "[VM] No script-created files found"
    fi
else
    if [ "$VNC_DEBUG" = "1" ]; then
        echo "[VM] No user lifecycle script found"
    fi
fi

exit 0 # Ensure the entry point script exits cleanly
